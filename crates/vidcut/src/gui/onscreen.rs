use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, Stroke, Vec2, pos2, vec2};
use serde_json::{Value, json};
use vidcut_core::lru::Lru;
use vidcut_core::model::{Project, Title, TitleKind, TrackKind};
use vidcut_core::render::Run;

use super::timeline::tools::landing;
use super::workspace::Workspace;

const GRAB: f32 = 6.0;
const SNAP: f32 = 8.0;
const LIFT: f32 = 26.0;
const LINES: [f64; 3] = [1.0 / 3.0, 0.5, 2.0 / 3.0];
const KEPT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hold {
    Move,
    Zoom,
    Turn,
}

#[derive(Debug, Clone)]
struct Held {
    clip: String,
    hold: Hold,
    from: Pos2,
    title: Title,
    now: Title,
    guides: (Option<f64>, Option<f64>),
}

#[derive(Debug, Default)]
pub struct Grab {
    selected: Option<String>,
    held: Option<Held>,
    landing: Option<(String, Title, std::time::Instant, Title)>,
    sent: Option<(String, Title)>,
    boxes: Lru<(Title, (u32, u32)), [f32; 4], KEPT>,
}

struct Shown {
    clip: String,
    title: Title,
    corners: [Pos2; 4],
    pivot: Pos2,
    angle: f32,
    turn_at: Pos2,
}

impl Shown {
    fn within(&self, p: Pos2, margin: f32) -> bool {
        let (sin, cos) = self.angle.sin_cos();
        let d = p - self.pivot;
        let back = self.pivot + vec2(d.x * cos - d.y * sin, d.x * sin + d.y * cos);
        let upright = |q: Pos2| {
            let d = q - self.pivot;
            self.pivot + vec2(d.x * cos - d.y * sin, d.x * sin + d.y * cos)
        };
        let r = Rect::from_points(&self.corners.map(upright)).expand(margin);
        r.contains(back)
    }

    fn inside(&self, p: Pos2) -> bool {
        self.within(p, GRAB)
    }

    fn near(&self, p: Pos2) -> bool {
        self.within(p, LIFT + GRAB * 1.6)
    }

    fn grip(&self, p: Pos2) -> Option<Hold> {
        let handles = self.title.kind != TitleKind::Scroll;
        if handles && self.turn_at.distance(p) <= GRAB * 1.6 {
            Some(Hold::Turn)
        } else if handles && self.corners.iter().any(|c| c.distance(p) <= GRAB * 1.6) {
            Some(Hold::Zoom)
        } else {
            self.inside(p).then_some(Hold::Move)
        }
    }

    fn cursor(&self, hold: Hold, p: Pos2) -> CursorIcon {
        match hold {
            Hold::Move => CursorIcon::Move,
            Hold::Turn => CursorIcon::PointingHand,
            Hold::Zoom => {
                let d = p - self.corners[0].lerp(self.corners[2], 0.5);
                if d.x * d.y > 0.0 {
                    CursorIcon::ResizeNwSe
                } else {
                    CursorIcon::ResizeNeSw
                }
            }
        }
    }
}

fn showing(p: &Project, at: i64) -> Vec<(String, Title, i64, i64)> {
    p.tracks
        .iter()
        .rev()
        .filter(|t| t.kind == TrackKind::Video && t.video_enabled)
        .flat_map(|t| {
            t.clips
                .iter()
                .filter(|c| c.enabled && c.start <= at && at < c.end())
                .filter_map(|c| Some((c.id.clone(), c.title.clone()?, c.start, c.duration)))
        })
        .collect()
}

fn rounded(v: f64, places: i32) -> f64 {
    let k = 10f64.powi(places);
    (v * k).round() / k
}

fn angle_of(v: Vec2) -> f32 {
    (-v.y).atan2(v.x)
}

impl Grab {
    fn upright_box(&mut self, t: &Title, size: (u32, u32)) -> Option<[f32; 4]> {
        let key = (t.clone(), size);
        if let Some(b) = self.boxes.get(&key) {
            return Some(*b);
        }
        let upright = Title {
            rotation: None,
            style: vidcut_core::model::TitleStyle {
                shadow: None,
                ..t.style.clone()
            },
            ..t.clone()
        };
        let s = vidcut_av::burn::title(&upright, size.0, size.1)
            .into_iter()
            .next()?;
        let b = [s.x as f32, s.y as f32, s.width as f32, s.height as f32];
        self.boxes.insert(key, b);
        Some(b)
    }
}

impl Workspace {
    pub(super) fn stored_title(&self, clip: &str) -> Option<Title> {
        self.project()?.clip(clip)?.title.clone()
    }

    fn title_wanted(&self) -> Option<(String, Title)> {
        let g = &self.timeline.title_grab;
        if let Some(h) = &g.held {
            return Some((h.clip.clone(), h.now.clone()));
        }
        if let Some(p) = self
            .timeline
            .title_dialog
            .as_ref()
            .and_then(|d| d.previewing())
        {
            return Some(p);
        }
        let (clip, to, since, from) = g.landing.as_ref()?;
        landing(to, since.elapsed(), from, &self.stored_title(clip)?)
            .map(|t| (clip.clone(), t.clone()))
    }

    pub(super) fn title_landing(&mut self, clip: &str, to: &Title) {
        self.timeline.title_grab.landing = self.stored_title(clip).map(|from| {
            (
                clip.to_string(),
                to.clone(),
                std::time::Instant::now(),
                from,
            )
        });
    }

    pub(super) fn sync_title_preview(&mut self) {
        let wanted = self.title_wanted();
        let g = &mut self.timeline.title_grab;
        if wanted.is_none() {
            g.landing = None;
        }
        if wanted != g.sent {
            self.live.preview_title(wanted.clone());
            g.sent = wanted;
        }
    }

    fn title_shapes(&mut self, picture: Rect) -> Vec<Shown> {
        let Some(p) = self.project_shared() else {
            return Vec::new();
        };
        let at = self.snap.player(crate::live::PlayerId::Timeline).position;
        let size = (p.settings.width, p.settings.height);
        let (w, h) = (f64::from(size.0), f64::from(size.1));
        let scale = vec2(picture.width() / w as f32, picture.height() / h as f32);
        let wanted = self.title_wanted();
        let mut out = Vec::new();
        for (clip, stored, start, length) in showing(&p, at) {
            let title = match &wanted {
                Some((id, t)) if *id == clip => t.clone(),
                _ => stored,
            };
            let base = Title {
                position: None,
                zoom: None,
                rotation: None,
                opacity: None,
                ..title.clone()
            };
            let Some([bx, by, bw, bh]) = self.timeline.title_grab.upright_box(&base, size) else {
                continue;
            };
            let z = title.scale() as f32;
            let [qx, qy] = base.pivot().map(|v| v as f32);
            let [rx, ry] = title.pivot().map(|v| v as f32);
            let (fw, fh) = (w as f32, h as f32);
            let x = rx * fw + (bx - qx * fw) * z;
            let y = if title.moves() {
                by
            } else {
                ry * fh + (by - qy * fh) * z
            };
            let (bw, bh) = (bw * z, bh * z);
            let dy = if title.moves() {
                let run = Run {
                    at: at - start,
                    of: length,
                    held: false,
                };
                run.top(0, h, f64::from(bh)) as f32 - y
            } else {
                0.0
            };
            let frame = |fx: f32, fy: f32| picture.min + vec2(fx * scale.x, fy * scale.y);
            let [px, py] = title.pivot();
            let pivot = frame((px * w) as f32, (py * h) as f32);
            let angle = title.turn().to_radians() as f32;
            let (sin, cos) = angle.sin_cos();
            let turned = |q: Pos2| {
                let d = q - pivot;
                pivot + vec2(d.x * cos + d.y * sin, -d.x * sin + d.y * cos)
            };
            let corners = [
                frame(x, y + dy),
                frame(x + bw, y + dy),
                frame(x + bw, y + bh + dy),
                frame(x, y + bh + dy),
            ]
            .map(turned);
            let top_middle = Pos2::new(
                (corners[0].x + corners[1].x) / 2.0,
                (corners[0].y + corners[1].y) / 2.0,
            );
            let up = vec2(-sin, -cos) * LIFT;
            out.push(Shown {
                clip,
                title,
                corners,
                pivot,
                angle,
                turn_at: top_middle + up,
            });
        }
        out
    }

    fn moved(&self, held: &Held, to: Pos2, picture: Rect, shift: bool, free: bool) -> Held {
        let mut next = held.clone();
        let t0 = &held.title;
        match held.hold {
            Hold::Move => {
                let mut d = to - held.from;
                if shift {
                    if d.x.abs() >= d.y.abs() {
                        d.y = 0.0;
                    } else {
                        d.x = 0.0;
                    }
                }
                let [x0, y0] = t0.place();
                let mut x = x0 + f64::from(d.x / picture.width());
                let mut y = y0 + f64::from(d.y / picture.height());
                let mut guides = (None, None);
                if !free {
                    let near = |v: f64, span: f32| {
                        LINES
                            .into_iter()
                            .find(|l| ((v - l).abs() as f32) * span <= SNAP)
                    };
                    if let Some(l) = near(x, picture.width()) {
                        x = l;
                        guides.0 = Some(l);
                    }
                    if t0.kind != TitleKind::Scroll
                        && let Some(l) = near(y, picture.height())
                    {
                        y = l;
                        guides.1 = Some(l);
                    }
                }
                if t0.kind == TitleKind::Scroll {
                    y = y0;
                }
                next.now.position =
                    Some([rounded(x.clamp(0.0, 1.0), 4), rounded(y.clamp(0.0, 1.0), 4)]);
                next.guides = guides;
            }
            Hold::Zoom => {
                let pivot = self.pivot_on(t0, picture);
                let (d0, d1) = (held.from.distance(pivot), to.distance(pivot));
                if d0 > 1.0 {
                    let z = t0.scale() * f64::from(d1 / d0);
                    next.now.zoom = Some(rounded(z.clamp(0.25, 4.0), 3));
                }
            }
            Hold::Turn => {
                let pivot = self.pivot_on(t0, picture);
                let turned =
                    f64::from(angle_of(to - pivot) - angle_of(held.from - pivot)).to_degrees();
                let mut r = t0.turn() + turned;
                if shift {
                    r = (r / 15.0).round() * 15.0;
                }
                let r = (r + 180.0).rem_euclid(360.0) - 180.0;
                next.now.rotation = Some(rounded(r, 2));
            }
        }
        next
    }

    fn pivot_on(&self, t: &Title, picture: Rect) -> Pos2 {
        let [px, py] = t.pivot();
        picture.min + vec2(px as f32 * picture.width(), py as f32 * picture.height())
    }

    pub(super) fn title_controls(&mut self, ui: &egui::Ui, resp: &egui::Response, picture: Rect) {
        let shapes = self.title_shapes(picture);
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let (shift, free) = ui.input(|i| (i.modifiers.shift, i.modifiers.alt));
        let selected = self.timeline.title_grab.selected.clone();
        let grip = |p: Pos2| {
            shapes
                .iter()
                .filter(|s| Some(&s.clip) == selected.as_ref())
                .chain(&shapes)
                .find_map(|s| Some((s, s.grip(p)?)))
        };
        let cursor = match &self.timeline.title_grab.held {
            Some(h) => shapes
                .iter()
                .find(|s| s.clip == h.clip)
                .zip(pointer)
                .map(|(s, p)| s.cursor(h.hold, p)),
            None => pointer
                .filter(|_| resp.hovered())
                .and_then(|p| grip(p).map(|(s, hold)| s.cursor(hold, p))),
        };
        if let Some(c) = cursor {
            ui.ctx().set_cursor_icon(c);
        }
        if resp.drag_started()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
            && let Some((s, hold)) = grip(press)
        {
            self.timeline.title_grab.selected = Some(s.clip.clone());
            self.timeline.title_grab.held = Some(Held {
                clip: s.clip.clone(),
                hold,
                from: press,
                title: s.title.clone(),
                now: s.title.clone(),
                guides: (None, None),
            });
        }
        if let (Some(held), Some(to)) = (self.timeline.title_grab.held.clone(), pointer)
            && resp.dragged()
        {
            self.timeline.title_grab.held = Some(self.moved(&held, to, picture, shift, free));
        }
        if resp.drag_stopped()
            && let Some(held) = self.timeline.title_grab.held.take()
            && let Some(params) = changed(&held)
        {
            self.send("titles.set", params);
            self.title_landing(&held.clip, &held.now);
            if let Some(d) = self
                .timeline
                .title_dialog
                .as_mut()
                .filter(|d| d.clip() == held.clip)
            {
                d.placed(&held.now);
            }
        }
        if resp.clicked() {
            let hit = pointer.and_then(|p| shapes.iter().find(|s| s.inside(p)));
            self.timeline.title_grab.selected = hit.map(|s| s.clip.clone());
        }
        if resp.double_clicked()
            && let Some(id) = pointer
                .and_then(|p| shapes.iter().find(|s| s.inside(p)))
                .map(|s| s.clip.clone())
        {
            self.open_title(&id);
        }
        if let Some(h) = &self.timeline.title_grab.held
            && h.hold == Hold::Zoom
        {
            let zoom = format!("{:.0}%", h.now.scale() * 100.0);
            egui::Tooltip::always_open(
                ui.ctx().clone(),
                ui.layer_id(),
                resp.id,
                egui::PopupAnchor::Pointer,
            )
            .gap(12.0)
            .show(|ui| ui.add(egui::Label::new(zoom).extend()));
        }
        self.sync_title_preview();
        self.paint_title_controls(ui, picture, &shapes, pointer);
    }

    fn paint_title_controls(
        &self,
        ui: &egui::Ui,
        picture: Rect,
        shapes: &[Shown],
        pointer: Option<Pos2>,
    ) {
        let painter = ui.painter_at(picture);
        let accent = super::theme::colors().accent;
        let selected = self.timeline.title_grab.selected.as_deref();
        let held = self.timeline.title_grab.held.as_ref();
        for s in shapes {
            let chosen = Some(s.clip.as_str()) == selected;
            let hovered = pointer.is_some_and(|p| s.near(p));
            if !chosen && !hovered {
                continue;
            }
            let stroke = Stroke::new(if chosen { 1.5 } else { 1.0 }, accent);
            for i in 0..4 {
                painter.line_segment([s.corners[i], s.corners[(i + 1) % 4]], stroke);
            }
            if s.title.kind != TitleKind::Scroll {
                for c in s.corners {
                    painter.rect_filled(
                        Rect::from_center_size(c, Vec2::splat(GRAB * 1.4)),
                        1.0,
                        accent,
                    );
                }
                let top_middle = Pos2::new(
                    (s.corners[0].x + s.corners[1].x) / 2.0,
                    (s.corners[0].y + s.corners[1].y) / 2.0,
                );
                painter.line_segment([top_middle, s.turn_at], stroke);
                painter.circle_filled(s.turn_at, GRAB * 0.9, accent);
            }
        }
        if let Some(hd) = held {
            let guide = Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 140));
            if let Some(x) = hd.guides.0 {
                let gx = picture.min.x + x as f32 * picture.width();
                painter.line_segment([pos2(gx, picture.min.y), pos2(gx, picture.max.y)], guide);
            }
            if let Some(y) = hd.guides.1 {
                let gy = picture.min.y + y as f32 * picture.height();
                painter.line_segment([pos2(picture.min.x, gy), pos2(picture.max.x, gy)], guide);
            }
        }
    }
}

fn changed(held: &Held) -> Option<Value> {
    let (a, b) = (&held.title, &held.now);
    let mut params = json!({ "clip": held.clip });
    if a.position != b.position {
        params["position"] = json!(b.place());
    }
    if a.zoom != b.zoom {
        params["zoom"] = json!(b.scale());
    }
    if a.rotation != b.rotation {
        params["rotation"] = json!(b.turn());
    }
    params
        .as_object()
        .is_some_and(|o| o.len() > 1)
        .then_some(params)
}
