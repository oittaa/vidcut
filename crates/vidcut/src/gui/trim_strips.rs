use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Painter, Pos2, Rect, Sense, pos2, vec2,
};
use vidcut_core::model::{Clip, Project};
use vidcut_core::ops::filmstrip;

use super::theme;
use super::tiles::{self, TileKey};
use super::timeline::{EditSel, Side};
use super::workspace::{Workspace, fit};

const GAP: f32 = 18.0;

pub(super) fn band_height(area: Rect) -> f32 {
    2.0 * strip_height(area) + GAP
}

fn strip_height(area: Rect) -> f32 {
    (area.height() * 0.14).clamp(36.0, 72.0)
}

struct Strip<'a> {
    rect: Rect,
    clip: &'a Clip,
    outgoing: bool,
    shift: i64,
}

impl Workspace {
    pub(super) fn trim_strips(&mut self, ui: &mut egui::Ui, band: Rect, p: &Project, e: EditSel) {
        ui.allocate_rect(band, Sense::hover());
        let Some(t) = p.track(&e.track.id()) else {
            return;
        };
        let out = t.clips.iter().find(|c| c.end() == e.at);
        let inc = t.clips.iter().find(|c| c.start == e.at);
        let h = (band.height() - GAP) / 2.0;
        let aspect = p.settings.width as f32 / p.settings.height.max(1) as f32;
        let w = (h * aspect).max(8.0);
        let cut = band.center().x.round();
        let top = Rect::from_min_size(band.min, vec2(band.width(), h));
        let bottom =
            Rect::from_min_size(pos2(band.left(), band.bottom() - h), vec2(band.width(), h));
        let gap = Rect::from_x_y_ranges(band.x_range(), top.bottom()..=bottom.top());
        let grab = |rect: Rect, width: f32, label: &str| {
            let at = Rect::from_center_size(pos2(cut, rect.center().y), vec2(width, rect.height()));
            let r = ui
                .interact(at, ui.id().with(("trim-strip", label)), Sense::drag())
                .on_hover_cursor(CursorIcon::ResizeHorizontal)
                .on_hover_text(label);
            r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, label));
            r
        };
        let handles = [
            (Side::Out, out.map(|_| grab(top, 12.0, "Trim the Out"))),
            (Side::In, inc.map(|_| grab(bottom, 12.0, "Trim the In"))),
            (
                Side::Both,
                out.and(inc).map(|_| grab(gap, 24.0, "Roll the edit")),
            ),
        ];
        let reach = |side: Side| match (side, out, inc) {
            (Side::Out, Some(o), _) => Some(filmstrip::out_reach(p, o)),
            (Side::In, _, Some(i)) => Some(filmstrip::in_reach(p, i)),
            (Side::Both, Some(o), Some(i)) => Some(filmstrip::roll_reach(p, o, i)),
            _ => None,
        };
        let active = handles.iter().find_map(|(side, r)| {
            let r = r.as_ref().filter(|r| r.dragged() || r.drag_stopped())?;
            let (lo, hi) = reach(*side)?;
            let by = match r.total_drag_delta() {
                Some(d) => {
                    let by = ((d.x / w).round() as i64).clamp(lo, hi.max(lo));
                    ui.data_mut(|m| m.insert_temp(r.id, by));
                    by
                }
                None => ui.data_mut(|m| m.remove_temp::<i64>(r.id)).unwrap_or(0),
            };
            Some((*side, by, r.drag_stopped()))
        });
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
            }),
            inc.map(|clip| Strip {
                rect: bottom,
                clip,
                outgoing: false,
                shift: shift(Side::In),
            }),
        ]
        .into_iter()
        .flatten()
        {
            self.paint_strip(&painter, p, &strip, cut, w, ppp);
        }
        let mark = theme::colors().selection;
        for (rect, side, width, color) in [
            (top, Side::Out, 3.0, mark),
            (bottom, Side::In, 3.0, mark),
            (
                gap.shrink2(vec2(0.0, 2.0)),
                Side::Both,
                16.0,
                Color32::WHITE,
            ),
        ] {
            let shown = match side {
                Side::Out => out.is_some(),
                Side::In => inc.is_some(),
                _ => out.is_some() && inc.is_some(),
            };
            if shown {
                let x = cut + shift(side) as f32 * w;
                let r =
                    Rect::from_center_size(pos2(x, rect.center().y), vec2(width, rect.height()));
                painter.rect_filled(r, CornerRadius::same(2), color);
            }
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

    fn paint_strip(&self, painter: &Painter, p: &Project, s: &Strip, cut: f32, w: f32, ppp: f32) {
        let c = s.clip;
        let left = ((cut - s.rect.left()) / w).ceil() as i64 + 1;
        let right = ((s.rect.right() - cut) / w).ceil() as i64 + 1;
        let media = p
            .media(&c.media)
            .filter(|m| m.info.video.is_some() && c.video);
        let aspect = media
            .and_then(tiles::media_aspect)
            .unwrap_or(w / s.rect.height());
        let height = tiles::tile_height(s.rect.height() * ppp);
        let width = (height as f32 * aspect).round().max(1.0) as u32;
        let (first, last) = if s.outgoing {
            (1 - c.duration, s.shift)
        } else {
            (s.shift, c.duration - 1)
        };
        for f in filmstrip::strip(p, c, s.outgoing, -left..=right) {
            let x = cut + (f.number - i64::from(s.outgoing)) as f32 * w;
            let cell = Rect::from_min_size(pos2(x, s.rect.top()), vec2(w, s.rect.height()));
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
            if !(first..=last).contains(&f.number) {
                painter.rect_filled(cell, CornerRadius::ZERO, Color32::from_black_alpha(160));
            }
            let label = if f.number == 0 {
                "0".to_string()
            } else {
                format!("{:+}", f.number)
            };
            let at = cell.center_top() + vec2(0.0, 2.0);
            painter.text(
                at + vec2(1.0, 1.0),
                Align2::CENTER_TOP,
                &label,
                FontId::proportional(11.0),
                Color32::BLACK,
            );
            painter.text(
                at,
                Align2::CENTER_TOP,
                label,
                FontId::proportional(11.0),
                Color32::WHITE,
            );
        }
    }
}
