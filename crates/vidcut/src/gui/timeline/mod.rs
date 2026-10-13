//! The timeline area (spec §5, §8): the upper timeline (the whole program)
//! over the lower timeline (a zoomed detail view whose playhead is fixed in
//! the middle by default), with the Timeline Options and Actions menus.
//!
//! Every gesture ends in one request: see [`gesture`]. What a gesture would
//! do is drawn while dragging ([`paint::preview`]).

pub mod geometry;
pub mod gesture;
pub mod paint;
pub mod tools;

use std::time::Instant;

use eframe::egui::{self, Align2, Color32, FontId, Key, Pos2, Rect, RichText, Sense, pos2, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use vidcut_core::model::{Project, Track};
use vidcut_core::ops::{self, Edge, MAIN};

use crate::gui::workspace::{Workspace, dim};
use crate::gui::{keys, theme};
use crate::live::PlayerId;
use geometry::{Hit, NEW_TRACK_STRIP, Rows, Scale, Snap, hit_rows};
use gesture::{Ctx, Drag, Gesture, Preview};
use paint::{Assets, Look};

/// Which part of an edit is selected (V, U).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    /// The outgoing clip's Out.
    Out,
    /// The incoming clip's In.
    In,
    /// Both: a roll of the picture cut.
    Both,
    /// The J/L handle: the audio edit (D20).
    /// `EditSel::at` is where that audio edit sits.
    Audio,
}

/// A selected edit point (keyboard trimming with `,` and `.`), on the
/// track it was picked on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditSel {
    pub at: i64,
    pub side: Side,
    pub track: TrackRef,
}

/// Outgoing clip of the audio edit at timeline frame `at` (D14): its audio
/// ends there, and the next clip has audio and starts where the picture ends.
pub(crate) fn audio_edit_outgoing(t: &Track, at: i64) -> Option<String> {
    t.clips.windows(2).find_map(|w| {
        let (a, b) = (&w[0], &w[1]);
        let tail = a.audio.as_ref()?.tail;
        (a.end() == b.start && b.audio.is_some() && a.end() + tail == at).then(|| a.id.clone())
    })
}

/// Trims sent that haven't landed yet, and the project as they leave it.
/// The GUI shows that and builds on it until they land, so a trim never
/// shows the timeline as it was for a frame, and quick trims add up. A
/// request that fails leaves it for a second at most.
#[derive(Debug)]
pub struct SentTrim {
    /// The revision once all of them have landed: each makes one.
    until: u64,
    project: Project,
    since: std::time::Instant,
}

/// Whether the cut `e` selects is still on its track: an undo or someone
/// else's edit can take it away.
pub(crate) fn edit_exists(p: &Project, e: &EditSel) -> bool {
    let Some(t) = p.track(&e.track.id()) else {
        return false;
    };
    match e.side {
        Side::Audio => audio_edit_outgoing(t, e.at).is_some(),
        _ => t.clips.iter().any(|c| c.start == e.at || c.end() == e.at),
    }
}

/// Timeline frame of the audio edit on the picture cut at `picture`.
/// `picture` itself when the clips that meet there have no audio.
pub(crate) fn audio_at_cut(t: &Track, picture: i64) -> i64 {
    t.clips
        .windows(2)
        .find_map(|w| {
            let (a, b) = (&w[0], &w[1]);
            (a.end() == picture && b.start == picture)
                .then(|| a.audio.as_ref().map_or(picture, |x| a.end() + x.tail))
        })
        .unwrap_or(picture)
}

/// A track by its id (`V2`, `A1`), small enough to keep selections `Copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackRef {
    pub audio: bool,
    pub n: u16,
}

impl TrackRef {
    /// Track 1, the main story: the Smart Indicator, V and Smart Insert
    /// take their edits from it (spec D36).
    pub const MAIN: Self = Self { audio: false, n: 1 };

    pub fn of(id: &str) -> Option<Self> {
        let (kind, n) = id.split_at_checked(1)?;
        Some(Self {
            audio: match kind {
                "V" => false,
                "A" => true,
                _ => return None,
            },
            n: n.parse().ok()?,
        })
    }

    /// The track of clip `id`.
    pub fn of_clip(p: &Project, id: &str) -> Option<Self> {
        let (t, _) = p.find_clip(id)?;
        Self::of(&p.tracks[t].id)
    }

    pub fn id(self) -> String {
        format!("{}{}", if self.audio { 'A' } else { 'V' }, self.n)
    }
}

/// Timeline Options, kept between runs.
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
    /// The Boring Detector is on, and its limits: clips longer
    /// than `boring_secs` and shorter than `jump_frames` are highlighted.
    pub boring: bool,
    pub boring_secs: f64,
    pub jump_frames: i64,
    /// What Dynamic Trim does with selected clips.
    pub dynamic_mode: tools::DynamicMode,
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
            // Decision D31.
            boring: false,
            boring_secs: 30.0,
            jump_frames: 10,
            dynamic_mode: tools::DynamicMode::Slip,
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
    ///.
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
    /// Ripple Delete Silence's dialog, while open.
    pub silence: Option<tools::SilenceDialog>,
    /// Analyze Timeline Edits (the Boring Detector's settings): seconds and
    /// frames being edited.
    pub boring_dialog: Option<(f64, i64)>,
    /// Dynamic Trim mode (W), while on.
    pub dynamic: Option<tools::Dynamic>,
    /// Upper and lower track rects and scales of the last frame.
    pub layout: Option<Layout>,
    /// How many tracks the project had in the last frame.
    pub tracks: usize,
    /// The project revision of the last frame.
    pub rev: u64,
    /// Trims sent that haven't landed yet.
    pub sent_trim: Option<SentTrim>,
    /// The track shown enlarged in the lower timeline (decision D43), if
    /// any.
    pub enlarged: Option<String>,
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
            enlarged: None,
            silence: None,
            boring_dialog: None,
            dynamic: None,
            layout: None,
            tracks: 0,
            rev: 0,
            sent_trim: None,
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
#[derive(Debug, Clone)]
pub struct Layout {
    pub upper_ruler: Rect,
    /// The upper timeline's rows: Track 1 and a thin row per other track.
    pub upper: Rows,
    pub upper_scale: Scale,
    pub lower_ruler: Rect,
    /// The lower timeline's rows, one per track.
    pub lower: Rows,
    pub lower_scale: Scale,
}

const UPPER_RULER: f32 = 16.0;
const UPPER_TRACK: f32 = 30.0;
/// Upper timeline rows for tracks other than Track 1.
const UPPER_OTHER: f32 = 9.0;
/// Preferred lower timeline row heights: Track 1, other video, audio.
const LOWER_ROWS: [f32; 3] = [100.0, 60.0, 46.0];
const LOWER_RULER: f32 = 22.0;
/// The track header column: wide enough for a track's name with its four
/// buttons on one line, which must not reach into the timeline.
const HEADER: f32 = 120.0;

/// The icons of the timeline's controls.
mod icon {
    pub use egui_phosphor::regular::{
        ARROWS_IN_LINE_HORIZONTAL as RIPPLE, ARROWS_IN_LINE_VERTICAL as SHRINK,
        ARROWS_OUT_LINE_HORIZONTAL as DYNAMIC_TRIM, ARROWS_OUT_LINE_VERTICAL as ENLARGE,
        BOOKMARK_SIMPLE as MARKER, EYE as VIDEO_ON, EYE_SLASH as VIDEO_OFF,
        LIGHTNING as TIMELINE_ACTIONS, LOCK_SIMPLE as LOCKED, LOCK_SIMPLE_OPEN as UNLOCKED,
        MAGNET as SNAP, PENCIL_SIMPLE as EDIT_ACTIONS, SCISSORS as SPLIT,
        SLIDERS_HORIZONTAL as OPTIONS, SPEAKER_HIGH as AUDIO_ON, SPEAKER_SLASH as AUDIO_OFF,
    };
}

/// An icon button lit while `on`, read by `name` (accessibility, tests),
/// with `tip` on hover.
fn icon_toggle(ui: &mut egui::Ui, icon: &str, name: &str, on: bool, tip: &str) -> egui::Response {
    let r = ui.add(egui::Button::selectable(on, RichText::new(icon).size(15.0)));
    r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, name));
    r.on_hover_text(tip)
}

/// A track header's icon button at `at`, lit while `lit`, read by `name`.
fn header_button(
    ui: &mut egui::Ui,
    at: Rect,
    icon: &str,
    name: &str,
    lit: bool,
    tip: &str,
) -> egui::Response {
    let r = ui.put(
        at,
        egui::Button::selectable(lit, RichText::new(icon).size(13.0)),
    );
    r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, lit, name));
    r.on_hover_text(tip)
}

/// An icon that opens a menu, read and hovered as `name`.
fn icon_menu(ui: &mut egui::Ui, icon: &str, name: &str, menu: impl FnOnce(&mut egui::Ui)) {
    let r = ui
        .menu_button(RichText::new(icon).size(15.0), menu)
        .response;
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    r.on_hover_text(name);
}

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
            target: false,
        }
    }

    /// Whether `track` is where edit functions go: the one picked, else
    /// Track 1.
    fn is_target(&self, track: &str) -> bool {
        self.target.as_deref() == Some(track) || (self.target.is_none() && track == MAIN)
    }

    /// The project as shown: with the trims on their way.
    pub(crate) fn shown_project(&mut self) -> Option<Project> {
        let p = self.project()?;
        if let Some(s) = &self.timeline.sent_trim {
            if p.rev < s.until && s.since.elapsed() < std::time::Duration::from_secs(1) {
                return Some(s.project.clone());
            }
            self.timeline.sent_trim = None;
        }
        self.project().cloned()
    }

    /// Records a trim just sent, as `trimmed` leaves the project shown.
    pub(crate) fn trim_sent(&mut self, trimmed: Project) {
        let rev = self.project().map_or(0, |p| p.rev);
        let until = match &self.timeline.sent_trim {
            Some(s) if rev < s.until => s.until + 1,
            _ => rev + 1,
        };
        self.timeline.sent_trim = Some(SentTrim {
            until,
            project: trimmed,
            since: std::time::Instant::now(),
        });
    }

    pub(crate) fn timeline_panel(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.shown_project() else {
            return;
        };
        self.timeline_toolbar(ui, &p);
        let full = ui.available_rect_before_wrap();
        let others = p.tracks.len().saturating_sub(1).min(6) as f32;
        // The overview starts after the track header column, lined up with
        // the detailed timeline, and leaves only a sliver after the end:
        // the window's left edge is easy to miss, its right edge isn't.
        let upper_rect = Rect::from_min_size(
            pos2(full.left() + HEADER, full.top()),
            vec2(
                full.width() - HEADER,
                UPPER_RULER + UPPER_TRACK + UPPER_OTHER * others,
            ),
        );
        let lower_rect = Rect::from_min_max(pos2(full.left(), upper_rect.bottom() + 6.0), full.max);
        ui.painter().rect_filled(
            Rect::from_min_max(full.min, pos2(upper_rect.left() - 1.0, upper_rect.bottom())),
            0.0,
            theme::PANEL_DARK,
        );
        let upper_resp = ui.allocate_rect(upper_rect, Sense::click_and_drag());
        let lower_resp = ui.allocate_rect(lower_rect, Sense::click_and_drag());

        // Scales.
        let fps = p.settings.fps.as_f64();
        let end = p.end();
        let span = ((end as f64 * 1.01).max(fps * 10.0)).ceil() as i64;
        let upper_scale = Scale::fit(upper_rect.left(), upper_rect.width(), span);
        let content = Rect::from_min_max(
            pos2(lower_rect.left() + HEADER, lower_rect.top()),
            lower_rect.max,
        );
        let playhead = self.snap.timeline.position;
        let lower_scale = self.lower_scale(content, playhead);
        let upper_ruler =
            Rect::from_min_size(upper_rect.min, vec2(upper_rect.width(), UPPER_RULER));
        let upper = Rows::layout(
            &p,
            Rect::from_min_max(
                pos2(upper_rect.left(), upper_ruler.bottom() + 1.0),
                upper_rect.max,
            ),
            [UPPER_TRACK, UPPER_OTHER, UPPER_OTHER],
            4.0,
            0.0,
            None,
        );
        let lower_ruler = Rect::from_min_size(content.min, vec2(content.width(), LOWER_RULER));
        let lower = Rows::layout(
            &p,
            Rect::from_min_max(
                pos2(content.left(), lower_ruler.bottom() + 2.0),
                content.max,
            ),
            LOWER_ROWS,
            22.0,
            NEW_TRACK_STRIP,
            self.timeline.enlarged.as_deref(),
        );
        let layout = Layout {
            upper_ruler,
            upper,
            upper_scale,
            lower_ruler,
            lower,
            lower_scale,
        };
        self.timeline.layout = Some(layout.clone());

        // Draw. While Dynamic Trim runs, the clips are drawn as the trim
        // leaves them, so the cut moves as it runs; K sends it (spec D32).
        let trimmed = self.dynamic_drawn(&p);
        let (drawn, edit_at) = match &trimmed {
            Some((q, at)) => (q, *at),
            None => (&p, None),
        };
        let thumbs = std::mem::take(&mut self.thumbs);
        let tiles = self.tiles.take();
        let (waveforms, missing) = (self.snap.waveforms.clone(), self.snap.missing.clone());
        let assets = Assets {
            thumbs: &thumbs,
            waveforms: &waveforms,
            tiles: tiles.as_ref(),
            missing: &missing,
        };
        let look = self.look();
        let smart = (end > 0).then(|| ops::nearest_edit(drawn, playhead));
        let selected = self.snap.selection.clips.clone();
        {
            let painter = ui.painter_at(upper_rect);
            paint::ruler(&painter, upper_ruler, &upper_scale, drawn, smart, true);
            for row in &layout.upper.rows {
                let Some(t) = drawn.track(&row.track) else {
                    continue;
                };
                let compact = Look {
                    filmstrips: false,
                    waveforms: false,
                    durations: false,
                    names: look.names && row.track == MAIN,
                    target: self.is_target(&row.track),
                    ..look
                };
                paint::track(
                    &painter,
                    &row.rect,
                    &upper_scale,
                    drawn,
                    t,
                    &selected,
                    &assets,
                    compact,
                );
            }
            self.paint_boring(&painter, &layout.upper, &upper_scale, drawn);
            self.paint_silence(&painter, &layout.upper, &upper_scale, drawn);
            paint::playhead(
                &painter,
                upper_scale.xf(playhead),
                upper_ruler,
                layout.upper.span().1,
                self.playhead_color(),
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
            // Each track's header is a band beside its row, with the rows'
            // gaps between them, so a header reads as its track's.
            // The target track's band is lighter, with an accent edge.
            for row in &layout.lower.rows {
                let band = Rect::from_min_max(
                    pos2(lower_rect.left() + 1.0, row.rect.rect.top()),
                    pos2(content.left() - 2.0, row.rect.rect.bottom()),
                );
                if self.is_target(&row.track) {
                    painter.rect_filled(band, 2.0, theme::PANEL_TARGET);
                    painter.rect_filled(
                        Rect::from_min_size(band.min, vec2(3.0, band.height())),
                        2.0,
                        theme::ACCENT,
                    );
                } else {
                    painter.rect_filled(band, 2.0, theme::PANEL);
                }
            }
            paint::ruler(&painter, lower_ruler, &lower_scale, drawn, smart, false);
            for row in &layout.lower.rows {
                let Some(t) = drawn.track(&row.track) else {
                    continue;
                };
                let look = Look {
                    // Small rows show less.
                    filmstrips: look.filmstrips && row.rect.rect.height() >= 40.0,
                    durations: look.durations && row.rect.rect.height() >= 40.0,
                    target: self.is_target(&row.track),
                    ..look
                };
                paint::track(
                    &painter,
                    &row.rect,
                    &lower_scale,
                    drawn,
                    t,
                    &selected,
                    &assets,
                    look,
                );
            }
            if let Some(e) = self.timeline.edit
                && let Some(v1) = layout.lower.get(&e.track.id())
            {
                let x = lower_scale.xf(edit_at.unwrap_or(e.at));
                let (a, b) = match e.side {
                    Side::Out => (x - 6.0, x),
                    Side::In => (x, x + 6.0),
                    Side::Both | Side::Audio => (x - 4.0, x + 4.0),
                };
                // The J/L handle lives in the audio band (D20).
                let y = if e.side == Side::Audio {
                    egui::Rangef::new(v1.rect.audio_top, v1.rect.rect.bottom())
                } else {
                    v1.rect.rect.y_range()
                };
                // Red where a Dynamic Trim run stopped at a limit.
                let stopped = self
                    .timeline
                    .dynamic
                    .as_ref()
                    .is_some_and(|d| d.stopped.is_some());
                painter.rect_filled(
                    Rect::from_x_y_ranges(a..=b, y),
                    0.0,
                    if stopped {
                        theme::ERROR
                    } else {
                        Color32::from_rgb(90, 200, 90)
                    },
                );
            }
            self.paint_silence(&painter, &layout.lower, &lower_scale, drawn);
            paint::playhead(
                &painter,
                lower_scale.xf(playhead),
                lower_ruler,
                layout.lower.span().1,
                self.playhead_color(),
            );
        }
        self.thumbs = thumbs;
        self.tiles = tiles;
        self.track_headers(ui, lower_rect, &layout.lower, &p);
        let upper_box =
            Rect::from_min_max(full.min, pos2(upper_rect.left() - 1.0, upper_rect.bottom()));
        let lower_box = Rect::from_min_max(
            lower_rect.min,
            pos2(content.left() - 1.0, layout.lower_ruler.bottom()),
        );
        self.timeline_boxes(ui, upper_box, lower_box, &p);

        // While a trim drags, its pointer stays.
        if let Some(d) = &self.timeline.drag
            && let Some(pos) = ui.input(|i| i.pointer.latest_pos())
        {
            let pointer = match &d.drag.gesture {
                Gesture::Trim {
                    edge: Edge::Head, ..
                } => Some(paint::TrimPointer::Start),
                Gesture::Trim {
                    edge: Edge::Tail, ..
                } => Some(paint::TrimPointer::End),
                Gesture::Roll { .. } | Gesture::AudioRoll { .. } => Some(paint::TrimPointer::Roll),
                Gesture::Slip { .. } => {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    None
                }
                _ => None,
            };
            if let Some(kind) = pointer {
                ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                let painter = ui.ctx().layer_painter(egui::LayerId::new(
                    egui::Order::Tooltip,
                    egui::Id::new("timeline-pointer"),
                ));
                paint::trim_pointer(&painter, pos, kind);
            }
        }

        // The pointer shows what a drag would do.
        if self.timeline.drag.is_none()
            && let Some(pos) = ui.input(|i| i.pointer.hover_pos())
        {
            for (rows, scale, area) in [
                (&layout.upper, &upper_scale, upper_rect),
                (&layout.lower, &lower_scale, content),
            ] {
                // Only over the timeline itself: an edge's zone reaches past
                // the first clip's start, over the track header's buttons.
                if !area.contains(pos) {
                    continue;
                }
                let (track, what) = hit_rows(&p, rows, scale, pos);
                // The audio edit has no mark of its own: light it up while it
                // can be grabbed, so a J/L cut is found without guessing.
                if let Hit::AudioEdit { outgoing, .. } = &what
                    && let Some(row) = track.as_deref().and_then(|t| rows.get(t))
                    && let Some(c) = p.clip(outgoing)
                {
                    let x = scale.xf(c.end() + c.audio.as_ref().map_or(0, |a| a.tail));
                    let band = egui::Rangef::new(row.rect.audio_top, row.rect.rect.bottom());
                    ui.painter().rect_filled(
                        Rect::from_x_y_ranges(x - 3.0..=x + 3.0, band),
                        1.0,
                        Color32::from_rgb(255, 210, 60),
                    );
                }
                // The slip handle shows on the clip under the pointer, and
                // lights up while it can be grabbed.
                if let Hit::Clip(id) | Hit::Slip(id) = &what
                    && let Some(row) = track.as_deref().and_then(|t| rows.get(t))
                    && let Some(c) = p.clip(id)
                    && let Some(handle) =
                        geometry::slip_handle(&row.rect, scale.xf(c.start), scale.xf(c.end()))
                {
                    let grabbed = matches!(what, Hit::Slip(_));
                    let (fill, ink) = if grabbed {
                        (Color32::from_rgb(255, 210, 60), Color32::BLACK)
                    } else {
                        (Color32::from_black_alpha(150), Color32::WHITE)
                    };
                    let handle = handle.shrink2(vec2(1.0, 2.0));
                    ui.painter().rect_filled(handle, 3.0, fill);
                    ui.painter().text(
                        handle.center(),
                        egui::Align2::CENTER_CENTER,
                        "↔",
                        egui::FontId::proportional(11.0),
                        ink,
                    );
                }
                // The clip end a drag would trim, or both ends at a cut that
                // rolls, turn green, and the timeline draws its own pointer:
                // a bracket for each end that moves, the arrow on its side.
                let green = Color32::from_rgb(90, 220, 120);
                let ends: Vec<(f32, f32)> = match &what {
                    Hit::Edge(id, edge) => p
                        .clip(id)
                        .map(|c| match edge {
                            Edge::Head => (scale.xf(c.start), 1.0),
                            Edge::Tail => (scale.xf(c.end()), -1.0),
                        })
                        .into_iter()
                        .collect(),
                    Hit::Roll { outgoing, .. } => p
                        .clip(outgoing)
                        .map(|c| vec![(scale.xf(c.end()), -1.0), (scale.xf(c.end()), 1.0)])
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                if let Some(row) = track.as_deref().and_then(|t| rows.get(t)) {
                    let ys = row.rect.rect.y_range();
                    // Each end: a solid band at the edge and a wash into the
                    // clip, on the side that moves (`side`: 1 into the clip
                    // on the right, -1 into the one on the left).
                    for (x, side) in &ends {
                        let span = |w: f32| {
                            let far = x + side * w;
                            Rect::from_x_y_ranges(x.min(far)..=x.max(far), ys)
                        };
                        ui.painter()
                            .rect_filled(span(18.0), 0.0, green.gamma_multiply(0.25));
                        ui.painter().rect_filled(span(4.0), 0.0, green);
                    }
                }
                let pointer = match &what {
                    Hit::Edge(_, Edge::Head) => Some(paint::TrimPointer::Start),
                    Hit::Edge(_, Edge::Tail) => Some(paint::TrimPointer::End),
                    Hit::Roll { .. } | Hit::AudioEdit { .. } => Some(paint::TrimPointer::Roll),
                    _ => None,
                };
                if let Some(kind) = pointer {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                    let painter = ui.ctx().layer_painter(egui::LayerId::new(
                        egui::Order::Tooltip,
                        egui::Id::new("timeline-pointer"),
                    ));
                    paint::trim_pointer(&painter, pos, kind);
                } else if let Hit::Slip(_) = what {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
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
            let (rows, scale) = if d.upper {
                (&layout.upper, &layout.upper_scale)
            } else {
                (&layout.lower, &layout.lower_scale)
            };
            let painter = ui.painter_at(if d.upper { upper_rect } else { lower_rect });
            paint::preview(&painter, rows, scale, &p, &d.preview);
            self.drag_tooltip(ui, &d.preview);
        }
        self.marker_window(ui.ctx());
        self.duration_window(ui.ctx());
        self.silence_window(ui.ctx());
        self.boring_window(ui.ctx());
        self.dynamic_tick(ui.ctx());
    }

    /// The playhead turns yellow in Dynamic Trim mode.
    fn playhead_color(&self) -> Color32 {
        if self.timeline.dynamic.is_some() {
            Color32::from_rgb(240, 200, 40)
        } else {
            theme::PLAYHEAD
        }
    }

    /// The lower timeline's scale: the playhead fixed in the middle, or a
    /// scrolled view that pages when the playhead leaves it.
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

    /// The row above the timelines: the In/Out marks, zoom and what the
    /// timeline holds. Its controls are in the header column's boxes
    /// (`timeline_boxes`).
    fn timeline_toolbar(&mut self, ui: &mut egui::Ui, p: &Project) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Timeline").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(dim(format!(
                    "{} clips, {}",
                    p.tracks.iter().map(|t| t.clips.len()).sum::<usize>(),
                    self.tc(p.end())
                )));
                ui.separator();
                let o = &mut self.timeline.options;
                if ui
                    .small_button("+")
                    .on_hover_text(keys("Zoom in (Ctrl+wheel)"))
                    .clicked()
                {
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

    /// The header column's boxes, as the Cut page has them (spec §5): by the
    /// overview, the Timeline Options, Timeline Actions and Edit Actions
    /// menus and the target track for edit functions; by the lower ruler,
    /// Ripple On, Dynamic Trim, Split, Add Marker and Snap. Icons, read by
    /// their names; each says its name and key on hover.
    fn timeline_boxes(&mut self, ui: &mut egui::Ui, upper: Rect, lower: Rect, p: &Project) {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(upper.shrink(3.0)));
        child.set_clip_rect(upper.intersect(child.clip_rect()));
        child.spacing_mut().item_spacing = vec2(2.0, 2.0);
        child.spacing_mut().button_padding = vec2(4.0, 1.0);
        child.horizontal(|ui| {
            icon_menu(ui, icon::OPTIONS, "Timeline Options", |ui| {
                self.options_menu(ui, p)
            });
            icon_menu(ui, icon::TIMELINE_ACTIONS, "Timeline Actions", |ui| {
                self.timeline_actions_menu(ui, p)
            });
            icon_menu(ui, icon::EDIT_ACTIONS, "Edit Actions", |ui| {
                self.edit_actions_menu(ui)
            });
        });
        child.horizontal_wrapped(|ui| {
            for t in p.tracks.iter() {
                let on = self.is_target(&t.id);
                let r = ui.selectable_label(on, RichText::new(&t.id).small());
                // Read apart from the track headers' own labels.
                let name = format!("Target {}", t.id);
                r.widget_info(|| {
                    egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, &name)
                });
                let r = r.on_hover_text(format!("Edit functions put clips on {}", t.id));
                if r.clicked() {
                    self.target = (t.id != MAIN).then(|| t.id.clone());
                }
            }
        });
        let mut child =
            ui.new_child(egui::UiBuilder::new().max_rect(lower.shrink2(vec2(3.0, 1.0))));
        child.set_clip_rect(lower.intersect(child.clip_rect()));
        child.spacing_mut().item_spacing = vec2(1.0, 0.0);
        child.spacing_mut().button_padding = vec2(2.0, 0.0);
        child.horizontal_centered(|ui| self.timeline_tools(ui, p));
    }

    /// Ripple On, Dynamic Trim, Split, Add Marker and Snap.
    fn timeline_tools(&mut self, ui: &mut egui::Ui, p: &Project) {
        let ripple = p.settings.ripple_on;
        if icon_toggle(
            ui,
            icon::RIPPLE,
            "Ripple On",
            ripple,
            "Ripple On: Track 1 closes gaps on insert, delete, move and trim; hold Alt to trim without",
        )
        .clicked()
        {
            self.send("project.set", json!({ "ripple_on": !ripple }));
        }
        let r = icon_toggle(
            ui,
            icon::DYNAMIC_TRIM,
            "Dynamic Trim",
            self.timeline.dynamic.is_some(),
            "Dynamic Trim (W): J/K/L trim the selected edit, or slip or slide the selected clips, as it plays. Right-click for Slip or Slide",
        );
        if r.clicked() {
            self.toggle_dynamic();
        }
        r.context_menu(|ui| {
            let o = &mut self.timeline.options;
            let before = o.dynamic_mode;
            ui.radio_value(&mut o.dynamic_mode, tools::DynamicMode::Slip, "Slip");
            ui.radio_value(&mut o.dynamic_mode, tools::DynamicMode::Slide, "Slide");
            if o.dynamic_mode != before {
                o.save();
                ui.close();
            }
        });
        if icon_toggle(ui, icon::SPLIT, "Split", false, &keys("Split (Ctrl+B)")).clicked() {
            self.send(
                "edit.split",
                json!({ "at": self.snap.timeline.position, "clips": self.snap.selection.clips }),
            );
        }
        if icon_toggle(ui, icon::MARKER, "Marker", false, "Add Marker (M)").clicked() {
            self.send("markers.add", json!({ "at": self.snap.timeline.position }));
        }
        if icon_toggle(
            ui,
            icon::SNAP,
            "Snap",
            self.snapping(),
            "Snap (N); during a drag only for that drag",
        )
        .clicked()
        {
            self.timeline.options.snapping = !self.timeline.options.snapping;
            self.timeline.options.save();
        }
    }

    /// Timeline Options: how the timeline shows and the modes of the whole
    /// timeline.
    fn options_menu(&mut self, ui: &mut egui::Ui, p: &Project) {
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
        ui.separator();
        ui.checkbox(&mut o.boring, "Boring Detector")
            .on_hover_text("Highlights clips that are too long (gray) or too short (red)");
        let open_boring = ui.button("Analyze Timeline Edits…").clicked();
        if *o != before {
            o.save();
        }
        if open_boring {
            self.timeline.boring_dialog = Some((
                self.timeline.options.boring_secs,
                self.timeline.options.jump_frames,
            ));
            ui.close();
        }
    }

    /// Timeline Actions: what adds to the timeline or changes it as a whole.
    fn timeline_actions_menu(&mut self, ui: &mut egui::Ui, p: &Project) {
        if ui.button("Add Video Track").clicked() {
            self.send("track.add", json!({ "kind": "video" }));
            ui.close();
        }
        if ui.button("Add Audio Track").clicked() {
            self.send("track.add", json!({ "kind": "audio" }));
            ui.close();
        }
        ui.separator();
        let at = self.snap.timeline.position;
        if ui.button(keys("Split Clips  (Ctrl+B)")).clicked() {
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
        let selected = self.snap.selection.clips.clone();
        let with = |want: fn(&vidcut_core::model::Clip) -> bool| {
            selected.iter().any(|id| p.clip(id).is_some_and(want))
        };
        if ui
            .add_enabled(
                with(|c| c.audio.is_some()),
                egui::Button::new("Ripple Delete Silence…"),
            )
            .on_hover_text("Cut the silent parts out of the selected clips")
            .clicked()
        {
            self.open_silence(selected.clone());
            ui.close();
        }
        if ui
            .add_enabled(with(|c| c.video), egui::Button::new("Detect Scene Cuts"))
            .on_hover_text("Split the selected clips where their shots change")
            .clicked()
        {
            self.send("edit.detect_scene_cuts", json!({ "clips": selected }));
            ui.close();
        }
    }

    /// Edit Actions: what changes the clips at the playhead or selected.
    fn edit_actions_menu(&mut self, ui: &mut egui::Ui) {
        let at = self.snap.timeline.position;
        for (label, edge) in [
            ("Trim Start to Playhead", "head"),
            ("Trim End to Playhead", "tail"),
        ] {
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
                egui::Button::new(keys("Change Clip Duration…  (Ctrl+D)")),
            )
            .clicked()
        {
            self.timeline.duration_dialog = Some(String::new());
            ui.close();
        }
    }

    /// Track headers: the number picks the target track for
    /// edit functions; lock, mute and video enable; a menu to add and remove
    /// tracks.
    fn track_headers(&mut self, ui: &mut egui::Ui, lower_rect: Rect, rows: &Rows, p: &Project) {
        for row in &rows.rows {
            let Some(t) = p.track(&row.track) else {
                continue;
            };
            let r = row.rect.rect;
            let rect = Rect::from_min_max(
                pos2(lower_rect.left() + 2.0, r.top()),
                pos2(lower_rect.left() + HEADER - 4.0, r.bottom()),
            );
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            // Nothing in a header reaches past it, to be hovered or clicked
            // over a clip.
            child.set_clip_rect(rect.intersect(child.clip_rect()));
            child.spacing_mut().button_padding.x = 3.0;
            let is_target = self.is_target(&t.id);
            let label = t.name.clone().unwrap_or_else(|| t.id.clone());
            let parts = geometry::header_parts(rect);
            // A long name is cut short rather than the toggles.
            let mut name = child.new_child(
                egui::UiBuilder::new()
                    .max_rect(parts.name)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            let r = name
                .add(
                    egui::Button::selectable(is_target, RichText::new(&label).strong().small())
                        .truncate(),
                )
                .on_hover_text("The target track for edit functions");
            // A click makes the track the target, however often it comes;
            // V1, the default, is no target of its own.
            if r.clicked() {
                self.target = (t.id != MAIN).then(|| t.id.clone());
            }
            r.context_menu(|ui| {
                if ui.button("Add Video Track").clicked() {
                    self.send("track.add", json!({ "kind": "video" }));
                    ui.close();
                }
                if ui.button("Add Audio Track").clicked() {
                    self.send("track.add", json!({ "kind": "audio" }));
                    ui.close();
                }
                if t.id != MAIN {
                    let n = t.clips.len();
                    let text = if n == 0 {
                        "Remove Track".to_string()
                    } else {
                        format!("Remove Track and Its {n} Clip(s)")
                    };
                    if ui.button(text).clicked() {
                        self.send("track.remove", json!({ "track": t.id, "force": true }));
                        if self.target.as_deref() == Some(t.id.as_str()) {
                            self.target = None;
                        }
                        ui.close();
                    }
                }
            });
            // The enlarge button: one track at a time.
            let enlarged = self.timeline.enlarged.as_deref() == Some(t.id.as_str());
            let (glyph, tip) = if enlarged {
                (icon::SHRINK, "Back to its usual height")
            } else {
                (
                    icon::ENLARGE,
                    "Enlarge the track, to see its pictures and sound",
                )
            };
            let name = format!("Enlarge {}", t.id);
            if header_button(&mut child, parts.enlarge, glyph, &name, enlarged, tip).clicked() {
                self.timeline.enlarged = (!enlarged).then(|| t.id.clone());
            }
            // The icon says the state; the button is lit when it is out of
            // the ordinary: locked, muted, video off. Each has its own
            // column, so an audio track's lock and audio sit under the video
            // tracks'.
            let lock = if t.locked {
                (icon::LOCKED, "Unlock the track")
            } else {
                (icon::UNLOCKED, "Lock the track")
            };
            let sound = if t.muted {
                (icon::AUDIO_OFF, "Turn its audio back on")
            } else {
                (icon::AUDIO_ON, "Mute its audio")
            };
            let mut buttons = vec![
                (
                    "Lock",
                    t.locked,
                    lock,
                    json!({ "track": t.id, "locked": !t.locked }),
                ),
                (
                    "Audio",
                    t.muted,
                    sound,
                    json!({ "track": t.id, "muted": !t.muted }),
                ),
            ];
            if t.kind == vidcut_core::model::TrackKind::Video {
                let picture = if t.video_enabled {
                    (icon::VIDEO_ON, "Turn its video off")
                } else {
                    (icon::VIDEO_OFF, "Turn its video back on")
                };
                let change = json!({ "track": t.id, "video_enabled": !t.video_enabled });
                buttons.push(("Video", !t.video_enabled, picture, change));
            }
            for ((what, lit, (glyph, tip), change), at) in buttons.into_iter().zip(parts.toggles) {
                let name = format!("{what} {}", t.id);
                if header_button(&mut child, at, glyph, &name, lit, tip).clicked() {
                    self.send("track.set", change);
                }
            }
        }
    }

    fn ctx_for<'a>(
        &'a self,
        p: &'a Project,
        upper: bool,
        layout: &'a Layout,
        snap: Option<&'a Snap>,
    ) -> Ctx<'a> {
        Ctx {
            project: p,
            scale: if upper {
                layout.upper_scale
            } else {
                layout.lower_scale
            },
            rows: if upper { &layout.upper } else { &layout.lower },
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
        // The track headers are buttons: a drag from there trims nothing.
        if !upper && press.x < layout.lower_ruler.left() {
            return;
        }
        let mods = ui.input(|i| i.modifiers);
        let (ruler, rows, scale) = if upper {
            (layout.upper_ruler, &layout.upper, layout.upper_scale)
        } else {
            (layout.lower_ruler, &layout.lower, layout.lower_scale)
        };
        let fixed = !upper && self.timeline.options.fixed_playhead;
        let playhead = self.snap.timeline.position;
        let (track, what) = hit_rows(p, rows, &scale, press);
        let gesture = if ruler.expand2(vec2(0.0, 2.0)).contains(press) || track.is_none() && !upper
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
            let track = track.unwrap_or_else(|| MAIN.to_string());
            // Other tracks ripple only where Track 1 has nothing.
            let ripples = |clip: &str| {
                track == MAIN
                    || p.clip(clip).is_some_and(|c| {
                        p.track(MAIN)
                            .is_none_or(|v1| v1.clips.iter().all(|x| x.end() <= c.start))
                    })
            };
            match what {
                Hit::Edge(clip, edge) => Gesture::Trim {
                    from: p
                        .clip(&clip)
                        .map_or(0, |c| if edge == Edge::Head { c.start } else { c.end() }),
                    ripple: p.settings.ripple_on && !mods.alt && ripples(&clip),
                    clip,
                    edge,
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
                        from: track,
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
        self.show_timeline();
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
        // A bin dropped on a folder or a path level. Self, a descendant
        // and the current parent send nothing (spec §3).
        if let Gesture::Bin { path } = &d.drag.gesture {
            let path = path.clone();
            if released {
                if let Some(to) = self.bin_rename_to(&path, pos) {
                    self.send("bins.rename", json!({ "path": path, "to": to }));
                }
                self.timeline.snap_flipped = false;
            } else {
                self.timeline.drag = Some(d);
                ui.ctx().request_repaint();
            }
            return;
        }
        if released
            && let Gesture::Media { media, .. } = &d.drag.gesture
            && let Some(bin) = self.bin_target_at(pos)
        {
            // Dropped on a bin in the pool: the clips move into it.
            let ids = if self.snap.selection.media.contains(media) {
                self.snap.selection.media.clone()
            } else {
                vec![media.clone()]
            };
            self.send("media.move", json!({ "media": ids, "bin": bin }));
            self.timeline.snap_flipped = false;
            return;
        }
        // Pool drags follow the pointer to whichever timeline it is over.
        if matches!(d.drag.gesture, Gesture::Media { .. }) {
            d.upper = layout.upper.area.expand(8.0).contains(pos);
        }
        let moving: Vec<String> = match &d.drag.gesture {
            Gesture::Move { clips, .. } => clips.clone(),
            _ => Vec::new(),
        };
        let scrubbing = matches!(d.drag.gesture, Gesture::Scrub { .. });
        let fixed = !d.upper && self.timeline.options.fixed_playhead;
        // Clips don't snap to a fixed playhead; scrubs don't snap
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
                self.send_drag(&method, r["params"].clone());
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
        let (ruler, rows, scale) = if upper {
            (layout.upper_ruler, &layout.upper, layout.upper_scale)
        } else {
            (layout.lower_ruler, &layout.lower, layout.lower_scale)
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
        let what = hit_rows(p, rows, &scale, pos).1;
        // egui counts a double-click as a triple when the click before
        // came shortly before, wherever it was: either opens.
        if resp.double_clicked() || resp.triple_clicked() {
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
                && let Some(track) = TrackRef::of_clip(p, outgoing)
            {
                self.open_trim_editor(c.end(), track);
            }
            return;
        }
        if resp.clicked() {
            self.show_timeline();
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
                    if let (Some(c), Some(track)) = (p.clip(id), TrackRef::of_clip(p, id)) {
                        let (at, side) = match edge {
                            Edge::Head => (c.start, Side::In),
                            Edge::Tail => (c.end(), Side::Out),
                        };
                        self.timeline.edit = Some(EditSel { at, side, track });
                    }
                }
                Hit::Roll { outgoing, .. } => {
                    if let (Some(c), Some(track)) =
                        (p.clip(outgoing), TrackRef::of_clip(p, outgoing))
                    {
                        self.timeline.edit = Some(EditSel {
                            at: c.end(),
                            side: Side::Both,
                            track,
                        });
                    }
                }
                // The J/L handle is the audio edit (D20). The selection sits
                // on that offset, so `,` and `.` move the audio.
                Hit::AudioEdit { outgoing, .. } => {
                    if let (Some(c), Some(track)) =
                        (p.clip(outgoing), TrackRef::of_clip(p, outgoing))
                    {
                        let at = c.end() + c.audio.as_ref().map_or(0, |a| a.tail);
                        self.timeline.edit = Some(EditSel {
                            at,
                            side: Side::Audio,
                            track,
                        });
                    }
                }
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
            && let Some(last) = sel.last().cloned()
            && let Some((ti, _)) = p.find_clip(id)
            && let Some(t) = p.tracks.get(ti)
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
        item(
            ui,
            &keys("Copy  (Ctrl+C)"),
            "edit.copy",
            json!({ "clips": clips }),
        );
        item(
            ui,
            &keys("Cut  (Ctrl+X)"),
            "edit.cut",
            json!({ "clips": clips }),
        );
        item(
            ui,
            "Delete  (Delete)",
            "edit.delete",
            json!({ "clips": clips }),
        );
        item(
            ui,
            &keys("Lift  (Alt+Delete)"),
            "edit.delete",
            json!({ "clips": clips, "lift": true }),
        );
        ui.separator();
        if ui.button(keys("Change Duration…  (Ctrl+D)")).clicked() {
            self.timeline.duration_dialog = Some(String::new());
            ui.close();
        }
        if c.audio.is_some() {
            ui.menu_button("Audio Operations", |ui| {
                if ui.button("Ripple Delete Silence…").clicked() {
                    self.open_silence(clips.clone());
                    ui.close();
                }
            });
        }
        if c.video && ui.button("Detect Scene Cuts").clicked() {
            self.send("edit.detect_scene_cuts", json!({ "clips": clips }));
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

    /// Starts dragging a bin onto another bin in the pool (spec §3).
    pub(crate) fn start_bin_drag(&mut self, path: &str, press: Pos2) {
        if path.is_empty() {
            return;
        }
        self.timeline.snap_flipped = false;
        self.timeline.drag = Some(ActiveDrag {
            drag: Drag::new(
                Gesture::Bin {
                    path: path.to_string(),
                },
                press,
            ),
            upper: false,
            preview: Preview::default(),
        });
    }

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
                    audio_only: self.audio_only,
                },
                press,
            ),
            upper: false,
            preview: Preview::default(),
        });
    }

    /// While media or a bin is being dragged anywhere: its label follows
    /// the pointer.
    pub(crate) fn media_drag_overlay(&self, ctx: &egui::Context) {
        if let Some(text) = self.bin_drag_label()
            && let Some(pos) = ctx.input(|i| i.pointer.latest_pos())
        {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                egui::Id::new("bin-drag"),
            ));
            let galley = painter.layout_no_wrap(text, FontId::proportional(12.0), Color32::WHITE);
            let at = pos + vec2(12.0, 10.0);
            painter.rect_filled(
                Rect::from_min_size(at - vec2(4.0, 2.0), galley.size() + vec2(8.0, 4.0)),
                3.0,
                theme::CLIP,
            );
            painter.galley(at, galley, Color32::WHITE);
            return;
        }
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

    /// Edit selection and keyboard trimming (`,`/`.` from the Edit
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
                track: TrackRef::MAIN,
            });
        }
        if pressed(Key::U)
            && let Some(e) = &mut self.timeline.edit
        {
            // U cycles the picture trim. An audio edit stays the audio edit (D20).
            e.side = match e.side {
                Side::Both => Side::Out,
                Side::Out => Side::In,
                Side::In => Side::Both,
                Side::Audio => Side::Audio,
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
        // While Dynamic Trim runs, its pending trim is the one edit (D32).
        if nudge != 0
            && self.timeline.dynamic.is_none()
            && let Some(e) = self.timeline.edit
        {
            self.nudge_edit(e.side, nudge);
        }
        if pressed(Key::D) && input.modifiers.command && !self.snap.selection.clips.is_empty() {
            self.timeline.duration_dialog = Some(String::new());
        }
        if pressed(Key::W) && !input.modifiers.command {
            self.toggle_dynamic();
        }
    }

    /// Trims the selected edit by `by` frames: the Out (outgoing tail), the
    /// In (incoming head), both (a picture roll), or the audio edit (the J/L
    /// offset). The selection follows the edit at once, ahead of the trim
    /// landing; a trim that can't be made (tried on a copy) leaves it, and
    /// the session says why.
    pub(crate) fn nudge_edit(&mut self, side: Side, by: i64) {
        let Some(e) = self.timeline.edit else { return };
        let Some(p) = self.shown_project() else {
            return;
        };
        let Some(t) = p.track(&e.track.id()) else {
            return;
        };
        if side == Side::Audio {
            let Some(id) = audio_edit_outgoing(t, e.at) else {
                return;
            };
            let mut q = p.clone();
            let tried = ops::audio_roll(&mut q, &id, Edge::Tail, by);
            self.send(
                "edit.audio_roll",
                json!({ "clip": id, "edge": "tail", "by": by }),
            );
            if tried.is_err() {
                return;
            }
            self.trim_sent(q);
            self.timeline.edit = Some(EditSel {
                at: e.at + by,
                side,
                track: e.track,
            });
            return;
        }
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
                false,
            ),
            _ => return,
        };
        let id = params["clip"].as_str().unwrap_or_default().to_string();
        let mut q = p.clone();
        let tried = match (method, &params["edge"]) {
            ("edit.roll", _) => ops::roll(&mut q, &id, Edge::Tail, by),
            (_, edge) if edge == "head" => ops::trim(&mut q, &id, Edge::Head, by, None),
            _ => ops::trim(&mut q, &id, Edge::Tail, by, None),
        };
        self.send(method, params);
        if tried.is_err() {
            return;
        }
        // Where an In trim leaves the edit depends on whether it ripples.
        let at = if moves {
            e.at + by
        } else {
            q.clip(&id).map_or(e.at, |c| c.start)
        };
        self.trim_sent(q);
        self.timeline.edit = Some(EditSel {
            at,
            side,
            track: e.track,
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

    /// Change Clip Duration (Ctrl+D).
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

    /// Opens the Trim Editor for the edit at `at` (double-click an edit).
    pub(crate) fn open_trim_editor(&mut self, at: i64, track: TrackRef) {
        self.timeline.edit = Some(EditSel {
            at,
            side: Side::Both,
            track,
        });
        self.trim_editor = Some(at);
        self.show_timeline();
    }
}
