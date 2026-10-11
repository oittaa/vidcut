//! The timeline area (spec §5, §8): the upper timeline (the whole program)
//! over the lower timeline (a zoomed detail view whose playhead is fixed in
//! the middle by default), with the Timeline Options and Actions menus.
//!
//! Every gesture ends in one request: see [`gesture`]. What a gesture would
//! do is drawn while dragging ([`paint::preview`]).

pub mod geometry;
pub mod gesture;
pub mod paint;

use std::time::Instant;

use eframe::egui::{self, Align2, Color32, FontId, Key, Pos2, Rect, RichText, Sense, pos2, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use vidcut_core::model::Project;
use vidcut_core::ops::{self, Edge, MAIN};

use crate::gui::theme;
use crate::gui::workspace::{Workspace, dim};
use crate::live::PlayerId;
use geometry::{Hit, Scale, Snap, TrackRect, hit};
use gesture::{Ctx, Drag, Gesture, Preview};
use paint::{Assets, Look};

/// Which part of an edit is selected (V, U, p. 720).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    /// The outgoing clip's Out.
    Out,
    /// The incoming clip's In.
    In,
    /// Both: a roll.
    Both,
}

/// A selected edit point (keyboard trimming with `,` and `.`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditSel {
    pub at: i64,
    pub side: Side,
}

/// Timeline Options (p. 645), kept between runs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub fixed_playhead: bool,
    pub snapping: bool,
    pub safe_edit: bool,
    pub names: bool,
    pub durations: bool,
    pub status: bool,
    pub filmstrips: bool,
    pub waveforms: bool,
    /// Lower timeline zoom in pixels per frame.
    pub ppf: f64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            fixed_playhead: true, // decision D1
            snapping: true,
            safe_edit: false,
            names: true,
            durations: true,
            status: true,
            filmstrips: true,
            waveforms: true,
            ppf: 4.0,
        }
    }
}

/// A gesture in progress, on the upper or lower timeline.
#[derive(Debug)]
pub struct ActiveDrag {
    pub drag: Drag,
    pub upper: bool,
    pub preview: Preview,
}

/// The timeline area's view state.
#[derive(Debug)]
pub struct TimelineView {
    pub options: Options,
    /// `Instant` of egui time zero.
    pub epoch: Instant,
    /// N pressed during the current drag: snapping flipped until release
    /// (p. 717).
    pub snap_flipped: bool,
    /// First visible frame of the lower timeline with a free playhead.
    pub scroll: f64,
    pub drag: Option<ActiveDrag>,
    pub edit: Option<EditSel>,
    /// The last frame a scrub showed (to send each frame once).
    pub scrub_sent: Option<i64>,
    /// Marker being edited, and its fields.
    pub marker_dialog: Option<MarkerDialog>,
    /// Change Clip Duration (Ctrl+D), the typed value.
    pub duration_dialog: Option<String>,
    /// Upper and lower track rects and scales of the last frame.
    pub layout: Option<Layout>,
}

impl Default for TimelineView {
    fn default() -> Self {
        Self {
            options: Options::default(),
            epoch: Instant::now(),
            snap_flipped: false,
            scroll: 0.0,
            drag: None,
            edit: None,
            scrub_sent: None,
            marker_dialog: None,
            duration_dialog: None,
            layout: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MarkerDialog {
    pub id: String,
    pub name: String,
    pub note: String,
    pub color: String,
}

/// Where things were drawn last frame (hit testing for new presses).
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub upper_ruler: Rect,
    pub upper: TrackRect,
    pub upper_scale: Scale,
    pub lower_ruler: Rect,
    pub lower: TrackRect,
    pub lower_scale: Scale,
}

const UPPER_RULER: f32 = 16.0;
const UPPER_TRACK: f32 = 30.0;
const LOWER_RULER: f32 = 22.0;
const HEADER: f32 = 58.0;

fn options_path() -> std::path::PathBuf {
    crate::paths::config_dir().join("timeline.json")
}

impl Options {
    pub fn load() -> Self {
        std::fs::read_to_string(options_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        if let Some(dir) = options_path().parent()
            && std::fs::create_dir_all(dir).is_ok()
        {
            let _ = std::fs::write(options_path(), json!(self).to_string());
        }
    }
}

impl Workspace {
    fn snapping(&self) -> bool {
        self.timeline.options.snapping != self.timeline.snap_flipped
    }

    fn look(&self) -> Look {
        let o = &self.timeline.options;
        Look {
            names: o.names,
            durations: o.durations,
            status: o.status,
            filmstrips: o.filmstrips,
            waveforms: o.waveforms,
        }
    }

    pub(crate) fn timeline_panel(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.project().cloned() else {
            return;
        };
        self.timeline_toolbar(ui, &p);
        let full = ui.available_rect_before_wrap();
        let upper_rect =
            Rect::from_min_size(full.min, vec2(full.width(), UPPER_RULER + UPPER_TRACK));
        let lower_rect = Rect::from_min_max(pos2(full.left(), upper_rect.bottom() + 6.0), full.max);
        let upper_resp = ui.allocate_rect(upper_rect, Sense::click_and_drag());
        let lower_resp = ui.allocate_rect(lower_rect, Sense::click_and_drag());

        // Scales.
        let fps = p.settings.fps.as_f64();
        let end = p.end();
        let span = ((end as f64 * 1.04).max(fps * 10.0)).ceil() as i64;
        let upper_scale = Scale::fit(upper_rect.left(), upper_rect.width(), span);
        let content = Rect::from_min_max(
            pos2(lower_rect.left() + HEADER, lower_rect.top()),
            lower_rect.max,
        );
        let playhead = self.snap.timeline.position;
        let lower_scale = self.lower_scale(content, playhead);
        let upper_ruler =
            Rect::from_min_size(upper_rect.min, vec2(upper_rect.width(), UPPER_RULER));
        let upper = TrackRect::new(
            Rect::from_min_max(
                pos2(upper_rect.left(), upper_ruler.bottom() + 1.0),
                upper_rect.max,
            ),
            0.3,
        );
        let lower_ruler = Rect::from_min_size(content.min, vec2(content.width(), LOWER_RULER));
        let lower_track = Rect::from_min_max(
            pos2(content.left(), lower_ruler.bottom() + 4.0),
            pos2(
                content.right(),
                (lower_ruler.bottom() + 4.0 + 110.0).min(content.bottom() - 2.0),
            ),
        );
        let lower = TrackRect::new(lower_track, 0.32);
        let layout = Layout {
            upper_ruler,
            upper,
            upper_scale,
            lower_ruler,
            lower,
            lower_scale,
        };
        self.timeline.layout = Some(layout);

        // Draw.
        let thumbs = std::mem::take(&mut self.thumbs);
        let waveforms = self.snap.waveforms.clone();
        let assets = Assets {
            thumbs: &thumbs,
            waveforms: &waveforms,
        };
        let look = self.look();
        let smart = (end > 0).then(|| ops::nearest_edit(&p, playhead));
        let selected = self.snap.selection.clips.clone();
        let v1 = p.track(MAIN).cloned();
        {
            let painter = ui.painter_at(upper_rect);
            paint::ruler(&painter, upper_ruler, &upper_scale, &p, smart, true);
            if let Some(t) = &v1 {
                let compact = Look {
                    filmstrips: false,
                    waveforms: false,
                    durations: false,
                    ..look
                };
                paint::track(
                    &painter,
                    &upper,
                    &upper_scale,
                    &p,
                    t,
                    &selected,
                    &assets,
                    compact,
                );
            }
            paint::playhead(
                &painter,
                upper_scale.xf(playhead),
                upper_ruler,
                upper.rect.bottom(),
            );
        }
        {
            let painter = ui.painter_at(lower_rect);
            painter.rect_filled(
                Rect::from_min_max(
                    lower_rect.min,
                    pos2(content.left() - 1.0, lower_rect.bottom()),
                ),
                0.0,
                theme::PANEL_DARK,
            );
            paint::ruler(&painter, lower_ruler, &lower_scale, &p, smart, false);
            if let Some(t) = &v1 {
                paint::track(
                    &painter,
                    &lower,
                    &lower_scale,
                    &p,
                    t,
                    &selected,
                    &assets,
                    look,
                );
            }
            if let Some(e) = self.timeline.edit {
                let x = lower_scale.xf(e.at);
                let (a, b) = match e.side {
                    Side::Out => (x - 6.0, x),
                    Side::In => (x, x + 6.0),
                    Side::Both => (x - 4.0, x + 4.0),
                };
                painter.rect_filled(
                    Rect::from_x_y_ranges(a..=b, lower.rect.y_range()),
                    0.0,
                    Color32::from_rgb(90, 200, 90),
                );
            }
            paint::playhead(
                &painter,
                lower_scale.xf(playhead),
                lower_ruler,
                lower.rect.bottom(),
            );
        }
        self.thumbs = thumbs;
        self.track_header(ui, lower_rect, lower, &p);

        // The pointer shows what a drag would do (p. 726–728).
        if self.timeline.drag.is_none()
            && let (Some(t), Some(pos)) = (&v1, ui.input(|i| i.pointer.hover_pos()))
        {
            for (tr, scale) in [(&upper, &upper_scale), (&lower, &lower_scale)] {
                if tr.rect.contains(pos) {
                    let icon = match hit(t, scale, tr, pos) {
                        Hit::Edge(..) => Some(egui::CursorIcon::ResizeHorizontal),
                        Hit::Roll { .. } => Some(egui::CursorIcon::ResizeColumn),
                        Hit::AudioEdit { .. } => Some(egui::CursorIcon::ResizeColumn),
                        Hit::Slip(_) => Some(egui::CursorIcon::Grab),
                        _ => None,
                    };
                    if let Some(i) = icon {
                        ui.ctx().set_cursor_icon(i);
                    }
                }
            }
        }

        // Input. Gesture timing (hover, Safe Edit) follows egui's clock, so
        // tests can drive it.
        let now =
            self.timeline.epoch + std::time::Duration::from_secs_f64(ui.input(|i| i.time).max(0.0));
        for (resp, is_upper) in [(&upper_resp, true), (&lower_resp, false)] {
            if resp.drag_started() {
                self.start_drag(ui, &p, is_upper, &layout);
            }
            self.clicks(ui, resp, &p, is_upper, &layout);
        }
        self.run_drag(ui, &p, &layout, now);
        if let Some(d) = &self.timeline.drag {
            let (tr, scale) = if d.upper {
                (&upper, &upper_scale)
            } else {
                (&lower, &lower_scale)
            };
            let painter = ui.painter_at(if d.upper { upper_rect } else { lower_rect });
            paint::preview(&painter, tr, scale, &p, &d.preview);
            self.drag_tooltip(ui, &d.preview);
        }
        self.marker_window(ui.ctx());
        self.duration_window(ui.ctx());
    }

    /// The lower timeline's scale: the playhead fixed in the middle, or a
    /// scrolled view that pages when the playhead leaves it (p. 716).
    fn lower_scale(&mut self, content: Rect, playhead: i64) -> Scale {
        let ppf = self.timeline.options.ppf;
        if self.timeline.options.fixed_playhead {
            return Scale::anchored(content.center().x, playhead as f64, ppf);
        }
        let visible = f64::from(content.width()) / ppf;
        let first = self.timeline.scroll;
        let p = playhead as f64;
        if p < first || p > first + visible * 0.98 {
            self.timeline.scroll = (p - visible * 0.05).max(0.0);
        }
        Scale::anchored(content.left(), self.timeline.scroll, ppf)
    }

    fn timeline_toolbar(&mut self, ui: &mut egui::Ui, p: &Project) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Timeline").strong());
            ui.menu_button("Options", |ui| {
                let mut ripple = p.settings.ripple_on;
                if ui.checkbox(&mut ripple, "Ripple On").changed() {
                    self.send("project.set", json!({ "ripple_on": ripple }));
                }
                let o = &mut self.timeline.options;
                let before = *o;
                ui.checkbox(&mut o.snapping, "Snap (N)");
                ui.checkbox(&mut o.fixed_playhead, "Fixed Playhead");
                ui.checkbox(&mut o.safe_edit, "Trim with Safe Edit");
                ui.separator();
                ui.checkbox(&mut o.names, "Display Clip Names");
                ui.checkbox(&mut o.durations, "Display Clip Durations");
                ui.checkbox(&mut o.status, "Display Clip Status");
                ui.checkbox(&mut o.filmstrips, "Filmstrips");
                ui.checkbox(&mut o.waveforms, "Waveforms");
                if *o != before {
                    o.save();
                }
            });
            ui.menu_button("Actions", |ui| {
                let at = self.snap.timeline.position;
                if ui.button("Split Clips  (Ctrl+\\)").clicked() {
                    self.send(
                        "edit.split",
                        json!({ "at": at, "clips": self.snap.selection.clips }),
                    );
                    ui.close();
                }
                if ui.button("Add Marker  (M)").clicked() {
                    self.send("markers.add", json!({ "at": at }));
                    ui.close();
                }
                ui.separator();
                for (label, edge) in [("Trim Start to Playhead", "head"), ("Trim End to Playhead", "tail")] {
                    if ui.button(label).clicked() {
                        self.send(
                            "edit.trim_to_playhead",
                            json!({ "edge": edge, "at": at, "clips": self.snap.selection.clips }),
                        );
                        ui.close();
                    }
                }
                if ui
                    .add_enabled(
                        !self.snap.selection.clips.is_empty(),
                        egui::Button::new("Change Clip Duration…  (Ctrl+D)"),
                    )
                    .clicked()
                {
                    self.timeline.duration_dialog = Some(String::new());
                    ui.close();
                }
            });
            let mut ripple = p.settings.ripple_on;
            if ui
                .toggle_value(&mut ripple, "Ripple On")
                .on_hover_text("Track 1 closes gaps on insert, delete, move and trim (p. 645); hold Alt to trim without")
                .changed()
            {
                self.send("project.set", json!({ "ripple_on": ripple }));
            }
            if ui.button("Split").on_hover_text("Ctrl+\\").clicked() {
                self.send(
                    "edit.split",
                    json!({ "at": self.snap.timeline.position, "clips": self.snap.selection.clips }),
                );
            }
            if ui.button("Marker").on_hover_text("M").clicked() {
                self.send("markers.add", json!({ "at": self.snap.timeline.position }));
            }
            let snap = self.snapping();
            if ui
                .selectable_label(snap, "Snap")
                .on_hover_text("N toggles; during a drag only for that drag")
                .clicked()
            {
                self.timeline.options.snapping = !self.timeline.options.snapping;
                self.timeline.options.save();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(dim(format!(
                    "{} clips, {}",
                    p.tracks.iter().map(|t| t.clips.len()).sum::<usize>(),
                    self.tc(p.end())
                )));
                ui.separator();
                let o = &mut self.timeline.options;
                if ui.small_button("+").on_hover_text("Zoom in (Ctrl+wheel)").clicked() {
                    o.ppf = (o.ppf * 1.5).min(40.0);
                    o.save();
                }
                if ui.small_button("−").on_hover_text("Zoom out").clicked() {
                    o.ppf = (o.ppf / 1.5).max(0.01);
                    o.save();
                }
                let marks = match (p.marks.mark_in, p.marks.mark_out) {
                    (None, None) => "no In/Out".to_string(),
                    (a, b) => format!(
                        "In {}  Out {}",
                        a.map_or("-".into(), |f| self.tc(f)),
                        b.map_or("-".into(), |f| self.tc(f - 1)),
                    ),
                };
                ui.label(dim(marks));
            });
        });
    }

    /// Track 1's header: lock, mute, video enable (pp. 647, 717).
    fn track_header(&mut self, ui: &mut egui::Ui, lower_rect: Rect, tr: TrackRect, p: &Project) {
        let Some(t) = p.track(MAIN) else { return };
        let rect = Rect::from_min_max(
            pos2(lower_rect.left() + 2.0, tr.rect.top()),
            pos2(lower_rect.left() + HEADER - 4.0, tr.rect.bottom()),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        child.label(RichText::new("1").strong());
        child.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (label, value, key, tip) in [
                ("L", t.locked, "locked", "Lock the track"),
                ("M", t.muted, "muted", "Mute its audio"),
                ("V", t.video_enabled, "video_enabled", "Show its video"),
            ] {
                if ui
                    .selectable_label(value, RichText::new(label).small())
                    .on_hover_text(tip)
                    .clicked()
                {
                    self.send("track.set", json!({ "track": "V1", key: !value }));
                }
            }
        });
    }

    fn ctx_for<'a>(
        &'a self,
        p: &'a Project,
        upper: bool,
        layout: &Layout,
        snap: Option<&'a Snap>,
    ) -> Ctx<'a> {
        Ctx {
            project: p,
            scale: if upper {
                layout.upper_scale
            } else {
                layout.lower_scale
            },
            track: if upper { layout.upper } else { layout.lower },
            snap: snap.filter(|_| self.snapping()),
            safe_edit: self.timeline.options.safe_edit,
            selected: &self.snap.selection.clips,
        }
    }

    /// A press that became a drag: pick the gesture from what is under it.
    fn start_drag(&mut self, ui: &egui::Ui, p: &Project, upper: bool, layout: &Layout) {
        let Some(press) = ui.input(|i| i.pointer.press_origin()) else {
            return;
        };
        let mods = ui.input(|i| i.modifiers);
        let (ruler, tr, scale) = if upper {
            (layout.upper_ruler, layout.upper, layout.upper_scale)
        } else {
            (layout.lower_ruler, layout.lower, layout.lower_scale)
        };
        let fixed = !upper && self.timeline.options.fixed_playhead;
        let playhead = self.snap.timeline.position;
        let gesture = if ruler.expand2(vec2(0.0, 2.0)).contains(press)
            || !tr.rect.contains(press) && !upper
        {
            let near = |f: i64| (scale.xf(f) - press.x).abs() <= 5.0;
            if let Some(k) = p.markers.iter().find(|k| near(k.at)) {
                Gesture::Marker {
                    id: k.id.clone(),
                    from: k.at,
                }
            } else if let Some(i) = p.marks.mark_in.filter(|&i| near(i)) {
                Gesture::Mark {
                    is_in: true,
                    from: i,
                }
            } else if let Some(o) = p.marks.mark_out.filter(|&o| near(o)) {
                Gesture::Mark {
                    is_in: false,
                    from: o,
                }
            } else {
                Gesture::Scrub {
                    drag_content: fixed,
                    from: playhead as f64,
                }
            }
        } else {
            let Some(t) = p.track(MAIN) else { return };
            match hit(t, &scale, &tr, press) {
                Hit::Edge(clip, edge) => Gesture::Trim {
                    from: p
                        .clip(&clip)
                        .map_or(0, |c| if edge == Edge::Head { c.start } else { c.end() }),
                    clip,
                    edge,
                    ripple: p.settings.ripple_on && !mods.alt,
                },
                Hit::Roll { outgoing, .. } => Gesture::Roll {
                    from: p.clip(&outgoing).map_or(0, |c| c.end()),
                    outgoing,
                },
                Hit::Slip(clip) => Gesture::Slip {
                    clips: if self.snap.selection.clips.contains(&clip) {
                        self.snap.selection.clips.clone()
                    } else {
                        vec![clip]
                    },
                },
                Hit::AudioEdit { outgoing, .. } => Gesture::AudioRoll {
                    from: p
                        .clip(&outgoing)
                        .map_or(0, |c| c.end() + c.audio.as_ref().map_or(0, |a| a.tail)),
                    outgoing,
                },
                Hit::Clip(id) => {
                    let mut clips = if self.snap.selection.clips.contains(&id) {
                        self.snap.selection.clips.clone()
                    } else {
                        // Dragging an unselected clip selects it.
                        self.send("selection.set", json!({ "clips": [id] }));
                        vec![id]
                    };
                    clips.sort_by_key(|c| p.clip(c).map_or(0, |c| c.start));
                    let start = clips.first().and_then(|c| p.clip(c)).map_or(0, |c| c.start);
                    let len = clips
                        .iter()
                        .filter_map(|c| p.clip(c))
                        .map(|c| c.duration)
                        .sum();
                    Gesture::Move {
                        clips,
                        grab: scale.frame(press.x),
                        start,
                        len,
                    }
                }
                Hit::Empty if upper => Gesture::Scrub {
                    drag_content: false,
                    from: playhead as f64,
                },
                Hit::Empty => Gesture::Marquee {
                    additive: mods.command,
                },
            }
        };
        if crate::live::debug() {
            eprintln!("[gesture] {gesture:?}");
        }
        self.send("viewer.set", json!({ "mode": "timeline" }));
        self.timeline.snap_flipped = false;
        self.timeline.scrub_sent = None;
        self.timeline.drag = Some(ActiveDrag {
            drag: Drag::new(gesture, press),
            upper,
            preview: Preview::default(),
        });
    }

    /// Advances the gesture in progress; the release sends its request.
    fn run_drag(&mut self, ui: &egui::Ui, p: &Project, layout: &Layout, now: Instant) {
        let Some(mut d) = self.timeline.drag.take() else {
            return;
        };
        let (pos, released, n, escape) = ui.input(|i| {
            (
                i.pointer.latest_pos(),
                i.pointer.any_released(),
                i.key_pressed(Key::N),
                i.key_pressed(Key::Escape),
            )
        });
        if escape {
            // Esc cancels: nothing changes (a scrub stays where it is).
            self.timeline.snap_flipped = false;
            if let Some(f) = self.timeline.scrub_sent.take() {
                self.seek(PlayerId::Timeline, f, false);
            }
            return;
        }
        if n {
            self.timeline.snap_flipped = !self.timeline.snap_flipped;
        }
        let Some(pos) = pos else {
            self.timeline.drag = Some(d);
            return;
        };
        // Pool drags follow the pointer to whichever timeline it is over.
        if matches!(d.drag.gesture, Gesture::Media { .. }) {
            d.upper = layout.upper.rect.expand(8.0).contains(pos);
        }
        let moving: Vec<String> = match &d.drag.gesture {
            Gesture::Move { clips, .. } => clips.clone(),
            _ => Vec::new(),
        };
        let scrubbing = matches!(d.drag.gesture, Gesture::Scrub { .. });
        let fixed = !d.upper && self.timeline.options.fixed_playhead;
        // Clips don't snap to a fixed playhead (p. 717); scrubs don't snap
        // to the playhead they move.
        let playhead = (!fixed && !scrubbing).then_some(self.snap.timeline.position);
        let snap = Snap::new(p, playhead, &moving);
        let (preview, request) = {
            let ctx = self.ctx_for(p, d.upper, layout, Some(&snap));
            let preview = d.drag.update(pos, now, &ctx);
            let request = if released {
                d.drag.finish(pos, now, &ctx)
            } else {
                None
            };
            (preview, request)
        };
        d.preview = preview;
        if let Some(f) = d.preview.seek
            && self.timeline.scrub_sent != Some(f)
        {
            self.timeline.scrub_sent = Some(f);
            self.seek(PlayerId::Timeline, f, !released);
        }
        if released {
            if let Some(r) = request {
                let method = r["method"].as_str().unwrap_or_default().to_string();
                self.send(&method, r["params"].clone());
            }
            if scrubbing && let Some(f) = self.timeline.scrub_sent.take() {
                self.seek(PlayerId::Timeline, f, false);
            }
            self.timeline.snap_flipped = false;
            return;
        }
        self.timeline.drag = Some(d);
        // Hover timers and auto-scroll need frames while dragging.
        ui.ctx().request_repaint();
    }

    /// Clicks that didn't become drags: select, seek, menus, dialogs.
    fn clicks(
        &mut self,
        ui: &mut egui::Ui,
        resp: &egui::Response,
        p: &Project,
        upper: bool,
        layout: &Layout,
    ) {
        let (ruler, tr, scale) = if upper {
            (layout.upper_ruler, layout.upper, layout.upper_scale)
        } else {
            (layout.lower_ruler, layout.lower, layout.lower_scale)
        };
        // Ctrl+wheel zooms the lower timeline.
        if !upper && resp.hovered() {
            let (zoom, scroll) = ui.input(|i| (i.modifiers.command, i.smooth_scroll_delta));
            if zoom && scroll.y != 0.0 {
                let o = &mut self.timeline.options;
                o.ppf = (o.ppf * f64::from((scroll.y / 200.0).exp())).clamp(0.01, 40.0);
                o.save();
            } else if !self.timeline.options.fixed_playhead && scroll.x != 0.0 {
                self.timeline.scroll =
                    (self.timeline.scroll - f64::from(scroll.x) / scale.ppf).max(0.0);
            }
        }
        let Some(pos) = resp.interact_pointer_pos() else {
            return;
        };
        let mods = ui.input(|i| i.modifiers);
        let on_ruler = ruler.expand2(vec2(0.0, 2.0)).contains(pos);
        let t = p.track(MAIN);
        let what = t.map_or(Hit::Empty, |t| hit(t, &scale, &tr, pos));
        if resp.double_clicked() {
            if on_ruler {
                if let Some(k) = p
                    .markers
                    .iter()
                    .find(|k| (scale.xf(k.at) - pos.x).abs() <= 5.0)
                {
                    self.open_marker(k.id.clone(), p);
                }
            } else if let Hit::Roll { outgoing, .. } | Hit::Edge(outgoing, Edge::Tail) = &what
                && let Some(c) = p.clip(outgoing)
            {
                self.open_trim_editor(c.end());
            }
            return;
        }
        if resp.clicked() {
            self.send("viewer.set", json!({ "mode": "timeline" }));
            if on_ruler {
                let f = scale.under(pos.x).clamp(0, p.end());
                if upper || !self.timeline.options.fixed_playhead {
                    self.seek(PlayerId::Timeline, f, false);
                }
                return;
            }
            match &what {
                Hit::Clip(id) | Hit::Slip(id) => self.click_clip(id, mods, p),
                Hit::Edge(id, edge) => {
                    if let Some(c) = p.clip(id) {
                        let (at, side) = match edge {
                            Edge::Head => (c.start, Side::In),
                            Edge::Tail => (c.end(), Side::Out),
                        };
                        self.timeline.edit = Some(EditSel { at, side });
                    }
                }
                Hit::Roll { outgoing, .. } => {
                    if let Some(c) = p.clip(outgoing) {
                        self.timeline.edit = Some(EditSel {
                            at: c.end(),
                            side: Side::Both,
                        });
                    }
                }
                Hit::AudioEdit { .. } => {}
                Hit::Empty => {
                    if !mods.command {
                        self.timeline.edit = None;
                        self.send("selection.set", json!({ "clips": [] }));
                    }
                    if upper {
                        self.seek(
                            PlayerId::Timeline,
                            scale.under(pos.x).clamp(0, p.end()),
                            false,
                        );
                    }
                }
            }
        }
        if resp.secondary_clicked()
            && let Hit::Clip(id) | Hit::Slip(id) | Hit::Edge(id, _) = &what
            && !self.snap.selection.clips.contains(id)
        {
            self.send("selection.set", json!({ "clips": [id] }));
        }
        let clip = match &what {
            Hit::Clip(id) | Hit::Slip(id) | Hit::Edge(id, _) => p.clip(id).cloned(),
            _ => None,
        };
        if let Some(c) = clip {
            resp.context_menu(|ui| self.clip_menu(ui, &c));
        }
    }

    fn click_clip(&mut self, id: &str, mods: egui::Modifiers, p: &Project) {
        let mut sel = self.snap.selection.clips.clone();
        if mods.command {
            match sel.iter().position(|c| c == id) {
                Some(i) => {
                    sel.remove(i);
                }
                None => sel.push(id.to_string()),
            }
        } else if mods.shift
            && let (Some(t), Some(last)) = (p.track(MAIN), sel.last().cloned())
        {
            let ids: Vec<&str> = t.clips.iter().map(|c| c.id.as_str()).collect();
            if let (Some(a), Some(b)) = (
                ids.iter().position(|c| *c == last),
                ids.iter().position(|c| *c == id),
            ) {
                sel = ids[a.min(b)..=a.max(b)]
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
            }
        } else {
            sel = vec![id.to_string()];
        }
        self.timeline.edit = None;
        self.send("selection.set", json!({ "clips": sel }));
    }

    fn clip_menu(&mut self, ui: &mut egui::Ui, c: &vidcut_core::model::Clip) {
        let clips = if self.snap.selection.clips.contains(&c.id) {
            self.snap.selection.clips.clone()
        } else {
            vec![c.id.clone()]
        };
        let item = |ui: &mut egui::Ui, label: &str, method: &str, params: Value| {
            if ui.button(label).clicked() {
                self.send(method, params);
                ui.close();
            }
        };
        item(
            ui,
            if c.enabled {
                "Disable  (D)"
            } else {
                "Enable  (D)"
            },
            "clip.set",
            json!({ "clips": clips, "enabled": !c.enabled }),
        );
        if let Some(a) = &c.audio {
            item(
                ui,
                if a.muted { "Unmute" } else { "Mute" },
                "clip.set",
                json!({ "clips": clips, "muted": !a.muted }),
            );
        }
        ui.separator();
        item(ui, "Copy  (Ctrl+C)", "edit.copy", json!({ "clips": clips }));
        item(ui, "Cut  (Ctrl+X)", "edit.cut", json!({ "clips": clips }));
        item(
            ui,
            "Delete  (Delete)",
            "edit.delete",
            json!({ "clips": clips }),
        );
        item(
            ui,
            "Lift  (Alt+Delete)",
            "edit.delete",
            json!({ "clips": clips, "lift": true }),
        );
        ui.separator();
        if ui.button("Change Duration…  (Ctrl+D)").clicked() {
            self.timeline.duration_dialog = Some(String::new());
            ui.close();
        }
    }

    fn drag_tooltip(&self, ui: &egui::Ui, pv: &Preview) {
        let Some((delta, duration)) = pv.delta else {
            return;
        };
        let Some(pos) = ui.input(|i| i.pointer.latest_pos()) else {
            return;
        };
        let text = match duration {
            Some(d) => format!("{delta:+}  ({})", self.src_tc(d)),
            None => format!("{delta:+}"),
        };
        let text = if pv.blocked {
            format!("{text}  ⏱ keep dragging to overwrite")
        } else {
            text
        };
        let painter = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("timeline-drag-tip"),
        ));
        let galley = painter.layout_no_wrap(text, FontId::monospace(11.0), Color32::WHITE);
        let at = pos + vec2(14.0, -24.0);
        painter.rect_filled(
            Rect::from_min_size(at - vec2(4.0, 2.0), galley.size() + vec2(8.0, 4.0)),
            3.0,
            Color32::from_black_alpha(200),
        );
        painter.galley(at, galley, Color32::WHITE);
    }

    // ---------------------------------------------------------------- media drags

    /// Starts dragging media from the pool (or the source viewer) towards the
    /// timeline (spec §7.3).
    pub(crate) fn start_media_drag(&mut self, media: &str, range: (i64, i64), press: Pos2) {
        if range.1 <= range.0 {
            return;
        }
        self.timeline.snap_flipped = false;
        self.timeline.drag = Some(ActiveDrag {
            drag: Drag::new(
                Gesture::Media {
                    media: media.to_string(),
                    range,
                    video_only: self.video_only,
                },
                press,
            ),
            upper: false,
            preview: Preview::default(),
        });
    }

    /// While media is being dragged anywhere: its label follows the pointer.
    pub(crate) fn media_drag_overlay(&self, ctx: &egui::Context) {
        let Some(d) = &self.timeline.drag else { return };
        let Gesture::Media { media, range, .. } = &d.drag.gesture else {
            return;
        };
        let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) else {
            return;
        };
        let name = self
            .project()
            .and_then(|p| p.media(media))
            .map_or(media.clone(), |m| m.name.clone());
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("media-drag"),
        ));
        let text = format!(
            "{}  {}",
            crate::gui::pool::elide(&name, 30),
            self.src_tc(range.1 - range.0)
        );
        let galley = painter.layout_no_wrap(text, FontId::proportional(12.0), Color32::WHITE);
        let at = pos + vec2(12.0, 10.0);
        painter.rect_filled(
            Rect::from_min_size(at - vec2(4.0, 2.0), galley.size() + vec2(8.0, 4.0)),
            3.0,
            theme::CLIP,
        );
        painter.galley(at, galley, Color32::WHITE);
        if d.preview.request.is_none() {
            painter.text(
                at + vec2(0.0, 20.0),
                Align2::LEFT_TOP,
                "drop on the timeline",
                FontId::proportional(10.0),
                theme::TEXT_DIM,
            );
        }
    }

    // ---------------------------------------------------------------- keys and dialogs

    /// Edit selection and keyboard trimming (p. 720; `,`/`.` from the Edit
    /// page nudge the selected edit one frame, Shift for one second).
    pub(crate) fn timeline_keys(&mut self, input: &egui::InputState) {
        let Some(p) = self.project().cloned() else {
            return;
        };
        let pressed = |k: Key| input.key_pressed(k);
        if pressed(Key::N) && self.timeline.drag.is_none() && !input.modifiers.command {
            self.timeline.options.snapping = !self.timeline.options.snapping;
            self.timeline.options.save();
        }
        if pressed(Key::V) && !input.modifiers.command && !input.modifiers.alt {
            let at = ops::nearest_edit(&p, self.snap.timeline.position);
            self.timeline.edit = Some(EditSel {
                at,
                side: Side::Both,
            });
        }
        if pressed(Key::U)
            && let Some(e) = &mut self.timeline.edit
        {
            e.side = match e.side {
                Side::Both => Side::Out,
                Side::Out => Side::In,
                Side::In => Side::Both,
            };
        }
        if input.modifiers.command && input.modifiers.shift && pressed(Key::A) {
            self.timeline.edit = None;
        }
        let step = if input.modifiers.shift {
            p.settings.fps.as_f64().round() as i64
        } else {
            1
        };
        let nudge = if pressed(Key::Comma) {
            -step
        } else if pressed(Key::Period) {
            step
        } else {
            0
        };
        if nudge != 0
            && let Some(e) = self.timeline.edit
        {
            self.nudge_edit(e.side, nudge);
        }
        if pressed(Key::D) && input.modifiers.command && !self.snap.selection.clips.is_empty() {
            self.timeline.duration_dialog = Some(String::new());
        }
    }

    /// Trims the selected edit by `by` frames: the Out (outgoing tail), the
    /// In (incoming head) or both (a roll). The selection follows the edit.
    pub(crate) fn nudge_edit(&mut self, side: Side, by: i64) {
        let Some(e) = self.timeline.edit else { return };
        let Some(p) = self.project().cloned() else {
            return;
        };
        let Some(t) = p.track(MAIN) else { return };
        let out = t.clips.iter().find(|c| c.end() == e.at);
        let inc = t.clips.iter().find(|c| c.start == e.at);
        let (method, params, moves) = match (side, out, inc) {
            (Side::Both, Some(o), Some(_)) => (
                "edit.roll",
                json!({ "clip": o.id, "edge": "tail", "by": by }),
                true,
            ),
            (Side::Out | Side::Both, Some(o), _) => (
                "edit.trim",
                json!({ "clip": o.id, "edge": "tail", "by": by }),
                true,
            ),
            (Side::In | Side::Both, _, Some(i)) => (
                "edit.trim",
                json!({ "clip": i.id, "edge": "head", "by": by }),
                // A rippled head trim keeps the clip's start.
                !p.settings.ripple_on,
            ),
            _ => return,
        };
        self.send(method, params);
        self.timeline.edit = Some(EditSel {
            at: if moves { e.at + by } else { e.at },
            side,
        });
    }

    pub(crate) fn open_marker(&mut self, id: String, p: &Project) {
        if let Some(k) = p.markers.iter().find(|k| k.id == id) {
            self.timeline.marker_dialog = Some(MarkerDialog {
                id,
                name: k.name.clone(),
                note: k.note.clone(),
                color: k.color.clone(),
            });
        }
    }

    fn marker_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.timeline.marker_dialog.take() else {
            return;
        };
        let mut open = true;
        let mut done = false;
        egui::Window::new("Marker")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                egui::Grid::new("marker-fields").show(ui, |ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut d.name);
                    ui.end_row();
                    ui.label("Notes");
                    ui.text_edit_multiline(&mut d.note);
                    ui.end_row();
                    ui.label("Color");
                    egui::ComboBox::from_id_salt("marker-color")
                        .selected_text(&d.color)
                        .show_ui(ui, |ui| {
                            for c in ops::MARKER_COLORS {
                                ui.selectable_value(&mut d.color, c.to_string(), *c);
                            }
                        });
                    ui.end_row();
                });
                ui.horizontal(|ui| {
                    if ui.button("Done").clicked() {
                        self.send(
                            "markers.set",
                            json!({ "marker": d.id, "name": d.name, "note": d.note, "color": d.color }),
                        );
                        done = true;
                    }
                    if ui.button("Remove Marker").clicked() {
                        self.send("markers.remove", json!({ "marker": d.id }));
                        done = true;
                    }
                });
            });
        if open && !done {
            self.timeline.marker_dialog = Some(d);
        }
    }

    /// Change Clip Duration (Ctrl+D, p. 693).
    fn duration_window(&mut self, ctx: &egui::Context) {
        let Some(mut text) = self.timeline.duration_dialog.take() else {
            return;
        };
        let clips = self.snap.selection.clips.clone();
        let mut open = !clips.is_empty();
        let mut done = false;
        egui::Window::new("Change Clip Duration")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(format!("{} clip(s)", clips.len()));
                let r = ui.add(
                    egui::TextEdit::singleline(&mut text)
                        .hint_text("frames (90f), seconds (3s) or a timecode")
                        .desired_width(220.0),
                );
                r.request_focus();
                let set = |d: Value| {
                    self.send(
                        "edit.change_duration",
                        json!({ "clips": clips, "duration": d }),
                    );
                };
                ui.horizontal(|ui| {
                    for s in [1, 5, 15] {
                        if ui.button(format!("{s} s")).clicked() {
                            set(json!(format!("{s}s")));
                            done = true;
                        }
                    }
                    if ui
                        .button("End")
                        .on_hover_text("To the end of the media")
                        .clicked()
                    {
                        set(json!("end"));
                        done = true;
                    }
                });
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    if let Some(spec) = crate::gui::viewer::timecode_input(&text) {
                        set(json!(spec));
                    }
                    done = true;
                }
            });
        if open && !done {
            self.timeline.duration_dialog = Some(text);
        }
    }

    /// Opens the Trim Editor for the edit at `at` (double-click an edit,
    /// p. 729).
    pub(crate) fn open_trim_editor(&mut self, at: i64) {
        self.timeline.edit = Some(EditSel {
            at,
            side: Side::Both,
        });
        self.trim_editor = Some(at);
        self.send("viewer.set", json!({ "mode": "timeline" }));
    }
}
