pub mod cues;
pub mod fades;
pub mod geometry;
pub mod gesture;
pub mod paint;
pub mod tools;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Color32, FontId, Key, Pos2, Rect, RichText, Sense, pos2, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use vidcut_core::model::{PictureTool, Project, Track};
use vidcut_core::ops::{self, Edge, MAIN};
use vidcut_core::request::ViewerMode;

use crate::gui::workspace::{Workspace, dim};
use crate::gui::{keys, theme};
use crate::live::PlayerId;
use geometry::{Hit, MARK_BAND, NEW_TRACK_STRIP, Rows, Scale, Snap, hit_rows};
use gesture::{Ctx, Drag, Gesture, HELD_FRAME, Preview, swipe};
use paint::{Assets, Look};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Out,
    In,
    Both,
    Audio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditSel {
    pub at: i64,
    pub side: Side,
    pub track: TrackRef,
}

pub(crate) fn repaint_in(ctx: &egui::Context, wait: Duration) {
    ctx.request_repaint_after(wait);
}

pub(crate) fn audio_edit_outgoing(t: &Track, at: i64) -> Option<String> {
    t.clips.windows(2).find_map(|w| {
        let (a, b) = (&w[0], &w[1]);
        let tail = a.audio.as_ref()?.tail;
        (a.end() == b.start && b.audio.is_some() && a.end() + tail == at).then(|| a.id.clone())
    })
}

#[derive(Debug)]
pub struct SentTrim {
    until: u64,
    project: Arc<Project>,
    since: std::time::Instant,
}

pub(crate) fn edit_exists(p: &Project, e: &EditSel) -> bool {
    let Some(t) = p.track(&e.track.id()) else {
        return false;
    };
    match e.side {
        Side::Audio => audio_edit_outgoing(t, e.at).is_some(),
        _ => t.clips.iter().any(|c| c.start == e.at || c.end() == e.at),
    }
}

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

fn audio_edits(t: &Track) -> impl Iterator<Item = (i64, i64)> + '_ {
    t.clips.windows(2).filter_map(|w| {
        let (a, b) = (&w[0], &w[1]);
        let tail = a.audio.as_ref()?.tail;
        (a.end() == b.start && b.audio.is_some()).then_some((a.end(), a.end() + tail))
    })
}

pub(crate) fn nearest_edit(t: &Track, near: i64, side: Side) -> Option<i64> {
    let points: Vec<(i64, i64)> = match side {
        Side::Out => t.clips.iter().map(|c| (c.end(), c.end())).collect(),
        Side::In => t.clips.iter().map(|c| (c.start, c.start)).collect(),
        Side::Both => t.edit_points().into_iter().map(|e| (e, e)).collect(),
        Side::Audio => audio_edits(t).collect(),
    };
    points
        .into_iter()
        .min_by_key(|&(cut, _)| ((cut - near).abs(), cut))
        .map(|(_, at)| at)
}

pub const ZOOM: std::ops::RangeInclusive<f64> = 0.01..=40.0;
pub const BORING_SECS: std::ops::RangeInclusive<f64> = 1.0..=300.0;
pub const JUMP_FRAMES: std::ops::RangeInclusive<i64> = 1..=120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackRef {
    pub audio: bool,
    pub n: u16,
}

impl TrackRef {
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

    pub fn of_clip(p: &Project, id: &str) -> Option<Self> {
        let (t, _) = p.find_clip(id)?;
        Self::of(&p.tracks[t].id)
    }

    pub fn id(self) -> String {
        format!("{}{}", if self.audio { 'A' } else { 'V' }, self.n)
    }
}

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
    pub ppf: f64,
    pub boring: bool,
    pub boring_secs: f64,
    pub jump_frames: i64,
    pub dynamic_mode: tools::DynamicMode,
    pub minimize_subtitles: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            fixed_playhead: true,
            snapping: true,
            safe_edit: false,
            names: true,
            durations: true,
            status: true,
            filmstrips: true,
            waveforms: true,
            ppf: 4.0,
            boring: false,
            boring_secs: 30.0,
            jump_frames: 10,
            dynamic_mode: tools::DynamicMode::Slip,
            minimize_subtitles: false,
        }
    }
}

#[derive(Debug)]
pub struct ActiveDrag {
    pub drag: Drag,
    pub upper: bool,
    pub preview: Preview,
}

#[derive(Debug)]
pub struct TimelineView {
    pub options: Options,
    pub epoch: Instant,
    pub snap_flipped: bool,
    pub scroll: f64,
    pub rows_lift: f32,
    pub drag: Option<ActiveDrag>,
    pub edit: Option<EditSel>,
    pub transition: Option<(TrackRef, i64)>,
    pub transition_menu: Option<bool>,
    pub fade_dialog: Option<fades::FadeDialog>,
    pub fade_in_frames: bool,
    pub clip_menu: Option<(bool, String)>,
    pub scrub_sent: Option<i64>,
    pub swipe: Option<(f64, Instant)>,
    pub marker_dialog: Option<MarkerDialog>,
    pub duration_dialog: Option<String>,
    pub speed_dialog: Option<SpeedDialog>,
    pub silence: Option<tools::SilenceDialog>,
    pub boring_dialog: Option<(f64, i64)>,
    pub dynamic: Option<tools::Dynamic>,
    pub strip_drag: Option<(EditSel, i64)>,
    pub landing: Option<(i64, Instant, i64)>,
    pub heard: bool,
    pub layout: Option<Layout>,
    pub tracks: usize,
    pub rev: u64,
    pub sent_trim: Option<SentTrim>,
    pub enlarged: Option<String>,
    pub placed: Option<(Arc<Project>, Arc<paint::Placed>)>,
    pub cue_menu: Option<cues::CueMenu>,
    pub(super) adding: Option<crate::gui::export::Ask>,
    pub cue_dialog: Option<crate::gui::subtitles::CueDialog>,
    pub(crate) cue_landings: Vec<crate::gui::subtitles::Landing>,
    pub(super) cue_listed: Option<String>,
    pub track_dialog: Option<crate::gui::subtitles::TrackDialog>,
    pub(super) adding_title: Option<crate::gui::export::Ask>,
    pub title_dialog: Option<crate::gui::titles::TitleDialog>,
    pub(crate) title_grab: crate::gui::onscreen::Grab,
    pub(crate) picture_grab: crate::gui::placing::Grab,
    pub(super) files: Option<crate::gui::subtitles::Files>,
    pub(super) retime: Option<crate::gui::retime::Retime>,
    pub(crate) subtitles_sent: Option<crate::gui::subtitles::SubtitlesKey>,
}

impl Default for TimelineView {
    fn default() -> Self {
        Self {
            options: Options::default(),
            epoch: Instant::now(),
            snap_flipped: false,
            scroll: 0.0,
            rows_lift: 0.0,
            drag: None,
            edit: None,
            transition: None,
            transition_menu: None,
            fade_dialog: None,
            fade_in_frames: false,
            clip_menu: None,
            scrub_sent: None,
            swipe: None,
            marker_dialog: None,
            duration_dialog: None,
            speed_dialog: None,
            enlarged: None,
            silence: None,
            boring_dialog: None,
            dynamic: None,
            strip_drag: None,
            landing: None,
            heard: false,
            layout: None,
            tracks: 0,
            rev: 0,
            sent_trim: None,
            placed: None,
            cue_menu: None,
            adding: None,
            cue_dialog: None,
            cue_landings: Vec::new(),
            cue_listed: None,
            track_dialog: None,
            adding_title: None,
            title_dialog: None,
            title_grab: Default::default(),
            picture_grab: Default::default(),
            files: None,
            retime: None,
            subtitles_sent: None,
        }
    }
}

impl TimelineView {
    fn placed_for(&mut self, p: &Arc<Project>) -> Arc<paint::Placed> {
        if let Some((q, placed)) = &self.placed
            && Arc::ptr_eq(q, p)
        {
            return placed.clone();
        }
        let placed = Arc::new(paint::placed(p));
        self.placed = Some((p.clone(), placed.clone()));
        placed
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpeedDialog {
    pub percent: f64,
    pub reverse: bool,
    pub freeze: bool,
    pub ripple: bool,
}

#[derive(Debug, Clone)]
pub struct MarkerDialog {
    pub id: String,
    pub name: String,
    pub note: String,
    pub color: String,
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub upper_ruler: Rect,
    pub upper: Rows,
    pub upper_scale: Scale,
    pub lower_ruler: Rect,
    pub lower: Rows,
    pub lower_scale: Scale,
}

const UPPER_RULER: f32 = 24.0;
const UPPER_TRACK: f32 = 40.0;
const UPPER_OTHER: f32 = 9.0;
const LOWER_ROWS: [f32; 3] = [130.0, 80.0, 60.0];
const LOWER_SUBTITLE: f32 = 40.0;
const LOWER_SUBTITLE_MINIMIZED: f32 = 22.0;
const LOWER_RULER: f32 = 30.0;
const HEADER: f32 = 150.0;
const BOX_CONTROL: f32 = 18.0;

mod icon {
    pub use egui_phosphor::regular::{
        ARROWS_IN_LINE_VERTICAL as SHRINK, ARROWS_OUT_LINE_VERTICAL as ENLARGE,
        BOOKMARK_SIMPLE as MARKER, EYE as VIDEO_ON, EYE_SLASH as VIDEO_OFF, LOCK_SIMPLE as LOCKED,
        LOCK_SIMPLE_OPEN as UNLOCKED, MAGNET as SNAP, SCISSORS as SPLIT, SPEAKER_HIGH as AUDIO_ON,
        SPEAKER_SLASH as AUDIO_OFF,
    };
}

fn icon_toggle(ui: &mut egui::Ui, icon: &str, name: &str, on: bool) -> egui::Response {
    let r = ui.add(egui::Button::selectable(on, RichText::new(icon).size(15.0)));
    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, name));
    r
}

fn header_button(ui: &mut egui::Ui, at: Rect, icon: &str, name: &str, lit: bool) -> egui::Response {
    let r = ui.put(
        at,
        egui::Button::selectable(lit, RichText::new(icon).size(13.0)),
    );
    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, lit, name));
    r
}

fn word_toggle(ui: &mut egui::Ui, word: &str, name: &str, on: bool) -> egui::Response {
    let r = ui.add(egui::Button::selectable(on, RichText::new(word).small()));
    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, name));
    r
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
            let _ = vidcut_core::temp::write(&options_path(), json!(self).to_string().as_bytes());
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

    fn is_target(&self, track: &str) -> bool {
        self.target.as_deref() == Some(track) || (self.target.is_none() && track == MAIN)
    }

    pub(crate) fn shown_project(&mut self) -> Option<Arc<Project>> {
        let p = self.project_shared()?;
        if let Some(s) = &self.timeline.sent_trim {
            if p.rev < s.until && s.since.elapsed() < std::time::Duration::from_secs(1) {
                return Some(s.project.clone());
            }
            self.timeline.sent_trim = None;
        }
        Some(p)
    }

    pub(crate) fn trim_sent(&mut self, trimmed: Project) {
        let rev = self.project().map_or(0, |p| p.rev);
        let until = match &self.timeline.sent_trim {
            Some(s) if rev < s.until => s.until + 1,
            _ => rev + 1,
        };
        self.timeline.sent_trim = Some(SentTrim {
            until,
            project: Arc::new(trimmed),
            since: std::time::Instant::now(),
        });
    }

    pub(crate) fn timeline_wanted(&mut self) -> Option<f32> {
        let p = self.shown_project()?;
        let others = (p.tracks.len().saturating_sub(1) + p.subtitles.len()).min(6) as f32;
        let [v1, video, audio] = LOWER_ROWS;
        let subtitle = if self.timeline.options.minimize_subtitles {
            LOWER_SUBTITLE_MINIMIZED
        } else {
            LOWER_SUBTITLE
        };
        let rows: f32 = p
            .tracks
            .iter()
            .map(|t| match t.kind {
                vidcut_core::model::TrackKind::Audio => audio,
                _ if t.id == MAIN => v1,
                _ => video,
            })
            .sum::<f32>()
            + subtitle * p.subtitles.len() as f32;
        Some(
            UPPER_RULER
                + UPPER_TRACK
                + UPPER_OTHER * others
                + 6.0
                + LOWER_RULER
                + 2.0
                + rows
                + NEW_TRACK_STRIP,
        )
    }

    pub(crate) fn timeline_panel(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.shown_project() else {
            return;
        };
        self.history_update(ui.ctx());
        self.timeline_toolbar(ui, &p);
        let trimmed = self.trim_drawn(&p);
        let before = match trimmed {
            None => self.history_shown(),
            Some(_) => None,
        };
        let shape: &Project = before.as_deref().unwrap_or(&p);
        let full = ui.available_rect_before_wrap();
        let others = (shape.tracks.len().saturating_sub(1) + shape.subtitles.len()).min(6) as f32;
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
            theme::colors().panel_dark,
        );
        let upper_resp = ui.allocate_rect(upper_rect, Sense::click_and_drag());
        upper_resp.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Unknown, true, "Timeline overview")
        });
        let lower_resp = ui.allocate_rect(lower_rect, Sense::click_and_drag());

        let fps = p.settings.fps.as_f64();
        let end = shape.end();
        let span = ((end as f64 * 1.01).max(fps * 10.0)).ceil() as i64;
        let upper_scale = Scale::fit(upper_rect.left(), upper_rect.width(), span);
        let content = Rect::from_min_max(
            pos2(lower_rect.left() + HEADER, lower_rect.top()),
            lower_rect.max,
        );
        let swallowed = self.history_swallows(ui, upper_rect.union(lower_rect), before.is_some());
        let marks = self.history_marks(ui.ctx());
        let playhead = match &trimmed {
            Some((_, at)) => *at,
            None => self.landed_playhead(),
        };
        let glided = match &trimmed {
            Some(_) => playhead as f64,
            None => self.glided_playhead(playhead),
        };
        let lower_scale = self.lower_scale(content, glided);
        if trimmed.is_none() {
            self.glide_repaint(ui.ctx(), upper_scale.ppf.max(lower_scale.ppf));
        }
        let lower_scale = match marks.as_ref().filter(|m| m.focus).and_then(focus) {
            Some((from, to))
                if lower_scale.xf(to) < content.left()
                    || lower_scale.xf(from) > content.right() =>
            {
                Scale::anchored(
                    content.center().x,
                    (from + to) as f64 / 2.0,
                    self.timeline.options.ppf,
                )
            }
            _ => lower_scale,
        };
        let upper_ruler =
            Rect::from_min_size(upper_rect.min, vec2(upper_rect.width(), UPPER_RULER));
        let upper = Rows::layout(
            shape,
            Rect::from_min_max(
                pos2(upper_rect.left(), upper_ruler.bottom() + 1.0),
                upper_rect.max,
            ),
            [UPPER_TRACK, UPPER_OTHER, UPPER_OTHER, UPPER_OTHER],
            4.0,
            0.0,
            None,
            0.0,
        );
        let lower_ruler = Rect::from_min_size(content.min, vec2(content.width(), LOWER_RULER));
        let [v1_h, video_h, audio_h] = LOWER_ROWS;
        let subtitle_h = if self.timeline.options.minimize_subtitles {
            LOWER_SUBTITLE_MINIMIZED
        } else {
            LOWER_SUBTITLE
        };
        let lower = Rows::layout(
            shape,
            Rect::from_min_max(
                pos2(content.left(), lower_ruler.bottom() + 2.0),
                content.max,
            ),
            [v1_h, video_h, audio_h, subtitle_h],
            22.0,
            NEW_TRACK_STRIP,
            self.timeline.enlarged.as_deref(),
            self.timeline.rows_lift,
        );
        self.timeline.rows_lift = self.timeline.rows_lift.min(lower.overflow(NEW_TRACK_STRIP));
        let rows_clip =
            Rect::from_min_max(pos2(lower_rect.left(), lower.area.top()), lower_rect.max);
        let layout = Layout {
            upper_ruler,
            upper,
            upper_scale,
            lower_ruler,
            lower,
            lower_scale,
        };
        self.timeline.layout = Some(layout.clone());

        let dragged = match (&trimmed, &before) {
            (None, None) => self
                .timeline
                .drag
                .as_ref()
                .and_then(|d| d.preview.moved(&p)),
            _ => None,
        };
        let (drawn, edit_at) = match (&trimmed, &before) {
            (Some((q, at)), _) => (q, self.timeline.edit.is_some().then_some(*at)),
            (None, Some(q)) => (&**q, None),
            (None, None) => (dragged.as_ref().unwrap_or(&*p), None),
        };
        let placed = match (&trimmed, &before) {
            (Some((q, _)), _) => Arc::new(paint::placed(q)),
            (None, Some(q)) => Arc::new(paint::placed(q)),
            (None, None) => self.timeline.placed_for(&p),
        };
        let none = Vec::new();
        let placed_on = |t: &Track| placed.get(&t.id).unwrap_or(&none);
        let pool = drawn.pool();
        let thumbs = std::mem::take(&mut self.thumbs);
        let tiles = self.tiles.take();
        let (waveforms, missing) = (self.snap.waveforms.clone(), self.snap.missing.clone());
        let transcribed = self.snap.transcribed.clone();
        let title_thumbs = self.title_thumbs.prepare(ui.ctx(), drawn).clone();
        let assets = Assets {
            thumbs: &thumbs,
            waveforms: &waveforms,
            tiles: tiles.as_ref(),
            missing: &missing,
            transcribed: &transcribed,
            pool: &pool,
            titles: &title_thumbs,
        };
        let look = self.look();
        let smart = (end > 0).then(|| ops::nearest_edit(drawn, playhead));
        let selected: HashSet<&str> = self
            .snap
            .selection
            .clips
            .iter()
            .map(String::as_str)
            .collect();
        let picked_cues: HashSet<&str> = self
            .snap
            .selection
            .cues
            .iter()
            .map(String::as_str)
            .collect();
        let typing: Vec<(&str, &str)> = self
            .timeline
            .cue_landings
            .iter()
            .map(crate::gui::subtitles::Landing::cue_text)
            .chain(
                self.timeline
                    .cue_dialog
                    .as_ref()
                    .map(|d| (d.cue.as_str(), d.text.as_str())),
            )
            .collect();
        {
            let painter = ui.painter_at(upper_rect);
            paint::ruler(&painter, upper_ruler, &upper_scale, drawn, smart, true);
            for row in &layout.upper.rows {
                if let Some(s) = drawn.subtitle_track(&row.track) {
                    cues::paint_track(
                        &painter,
                        &row.rect,
                        &upper_scale,
                        s,
                        &drawn.speakers,
                        &picked_cues,
                        true,
                        &typing,
                    );
                    continue;
                }
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
                    placed_on(t),
                );
            }
            self.paint_boring(&painter, &layout.upper, &upper_scale, drawn);
            self.paint_silence(&painter, &layout.upper, &upper_scale, drawn);
            if let Some(m) = &marks {
                paint::marks(&painter, &layout.upper, &upper_scale, drawn, m);
            }
            self.paint_edit_selection(
                ui.ctx(),
                &painter,
                &layout.upper,
                &upper_scale,
                drawn,
                edit_at,
                5.0,
            );
            paint::playhead(
                &painter,
                upper_scale.x(glided),
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
                theme::colors().panel_dark,
            );
            let ruler_painter = painter;
            let painter =
                ruler_painter.with_clip_rect(rows_clip.intersect(ruler_painter.clip_rect()));
            for row in &layout.lower.rows {
                let band = Rect::from_min_max(
                    pos2(lower_rect.left() + 1.0, row.rect.rect.top()),
                    pos2(content.left() - 2.0, row.rect.rect.bottom()),
                );
                if self.is_target(&row.track) {
                    painter.rect_filled(band, 2.0, theme::colors().panel_target);
                    painter.rect_filled(
                        Rect::from_min_size(band.min, vec2(3.0, band.height())),
                        2.0,
                        theme::colors().accent,
                    );
                } else {
                    painter.rect_filled(band, 2.0, theme::colors().panel);
                }
            }
            paint::ruler(
                &ruler_painter,
                lower_ruler,
                &lower_scale,
                drawn,
                smart,
                false,
            );
            for row in &layout.lower.rows {
                if let Some(s) = drawn.subtitle_track(&row.track) {
                    cues::paint_track(
                        &painter,
                        &row.rect,
                        &lower_scale,
                        s,
                        &drawn.speakers,
                        &picked_cues,
                        false,
                        &typing,
                    );
                    continue;
                }
                let Some(t) = drawn.track(&row.track) else {
                    continue;
                };
                let look = Look {
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
                    placed_on(t),
                );
            }
            self.paint_edit_selection(
                ui.ctx(),
                &painter,
                &layout.lower,
                &lower_scale,
                drawn,
                edit_at,
                10.0,
            );
            if let Some(e) = self.timeline.edit
                && self.timeline.transition == Some((e.track, e.at))
                && let Some(row) = layout.lower.get(&e.track.id())
                && let Some(x) = self.trim_transition(&p, e)
            {
                let r = Rect::from_x_y_ranges(
                    lower_scale.xf(e.at - x.before)..=lower_scale.xf(e.at + x.after),
                    row.rect.rect.top()..=row.rect.audio_top.max(row.rect.rect.top() + 4.0),
                );
                painter.rect_stroke(
                    r,
                    2.0,
                    egui::Stroke::new(2.0, theme::colors().error),
                    egui::StrokeKind::Inside,
                );
            }
            self.paint_silence(&painter, &layout.lower, &lower_scale, drawn);
            if let Some(m) = &marks {
                paint::marks(&painter, &layout.lower, &lower_scale, drawn, m);
            }
            paint::playhead(
                &ruler_painter,
                lower_scale.x(glided),
                lower_ruler,
                layout.lower.span().1.min(layout.lower.area.bottom()),
                self.playhead_color(),
            );
        }
        self.thumbs = thumbs;
        self.tiles = tiles;
        let mut headers = ui.new_child(egui::UiBuilder::new().max_rect(lower_rect));
        headers.set_clip_rect(rows_clip.intersect(ui.clip_rect()));
        self.track_headers(&mut headers, lower_rect, &layout.lower, &p);
        self.row_scrollbar(ui, &layout.lower);
        let upper_box =
            Rect::from_min_max(full.min, pos2(upper_rect.left() - 1.0, upper_rect.bottom()));
        let lower_box = Rect::from_min_max(
            lower_rect.min,
            pos2(content.left() - 1.0, layout.lower_ruler.bottom()),
        );
        self.timeline_boxes(ui, upper_box, lower_box, &p);

        if let Some(d) = &self.timeline.drag
            && let Some(pos) = ui.input(|i| i.pointer.latest_pos())
        {
            let pointer = match &d.drag.gesture {
                Gesture::Trim {
                    edge: Edge::Head, ..
                }
                | Gesture::CueTrim {
                    edge: Edge::Head, ..
                } => Some(paint::TrimPointer::Start),
                Gesture::Trim {
                    edge: Edge::Tail, ..
                }
                | Gesture::CueTrim {
                    edge: Edge::Tail, ..
                } => Some(paint::TrimPointer::End),
                Gesture::Roll { .. } | Gesture::AudioRoll { .. } | Gesture::Fade { .. } => {
                    Some(paint::TrimPointer::Roll)
                }
                Gesture::Slip { .. } => {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    None
                }
                Gesture::Mark { .. } | Gesture::Marker { .. } => {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    None
                }
                _ => None,
            };
            if let Some(kind) = pointer {
                ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                let painter = ui.ctx().layer_painter(egui::LayerId::new(
                    egui::Order::Tooltip,
                    egui::Id::unique("timeline-pointer"),
                ));
                paint::trim_pointer(&painter, pos, kind);
            }
        }

        let on_stripe = self.timeline.drag.is_none()
            && self.stripe_hover(ui, &p, &layout.lower, &layout.lower_scale);
        if self.timeline.drag.is_none()
            && before.is_none()
            && !on_stripe
            && let Some(pos) = ui.input(|i| i.pointer.hover_pos())
        {
            for (rows, scale, area, ruler) in [
                (&layout.upper, &upper_scale, upper_rect, upper_ruler),
                (&layout.lower, &lower_scale, content, lower_ruler),
            ] {
                if !area.contains(pos) {
                    continue;
                }
                if mark_grab(&p, ruler, scale, pos).is_some() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    continue;
                }
                if let Some(row) = rows.at(pos.y).filter(|r| geometry::is_subtitle(&r.track)) {
                    self.cue_hover(ui, &p, row, scale, pos);
                    continue;
                }
                let (track, what) = hit_rows(&p, rows, scale, pos);
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
                let on_fade = rows
                    .at(pos.y)
                    .and_then(|r| {
                        geometry::fade_edge_at(&p, p.track(&r.track)?, scale, &r.rect, pos)
                    })
                    .is_some();
                let pointer = match &what {
                    _ if on_fade => Some(paint::TrimPointer::Roll),
                    Hit::Edge(_, Edge::Head) => Some(paint::TrimPointer::Start),
                    Hit::Edge(_, Edge::Tail) => Some(paint::TrimPointer::End),
                    Hit::Roll { .. } | Hit::AudioEdit { .. } => Some(paint::TrimPointer::Roll),
                    _ => None,
                };
                if let Some(kind) = pointer {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                    let painter = ui.ctx().layer_painter(egui::LayerId::new(
                        egui::Order::Tooltip,
                        egui::Id::unique("timeline-pointer"),
                    ));
                    paint::trim_pointer(&painter, pos, kind);
                } else if let Hit::Slip(_) = what {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                }
            }
        }

        let now =
            self.timeline.epoch + std::time::Duration::from_secs_f64(ui.input(|i| i.time).max(0.0));
        for (resp, is_upper) in [(&upper_resp, true), (&lower_resp, false)] {
            if swallowed {
                continue;
            }
            let striped =
                !is_upper && self.stripe_press(ui, resp, &p, &layout.lower, &layout.lower_scale);
            if resp.drag_started() && !striped {
                self.start_drag(ui, &p, is_upper, &layout);
            }
            self.wheel(ui, resp, &p, is_upper, &layout, now);
            if !striped {
                self.clicks(ui, resp, &p, is_upper, &layout);
            }
        }
        self.scroll_rows(ui, rows_clip, &layout.lower);
        self.end_swipe(ui.ctx(), now);
        self.run_drag(ui, &p, &layout, now);
        self.stripe_drag(ui, &p, &layout.lower_scale);
        if let Some(d) = &self.timeline.drag {
            let (rows, scale) = if d.upper {
                (&layout.upper, &layout.upper_scale)
            } else {
                (&layout.lower, &layout.lower_scale)
            };
            let painter = ui.painter_at(if d.upper { upper_rect } else { rows_clip });
            paint::preview(&painter, rows, scale, &p, &d.preview);
            self.drag_tooltip(ui, &d.preview);
        }
        self.marker_window(ui.ctx());
        self.duration_window(ui.ctx());
        self.fade_window(ui.ctx());
        self.speed_window(ui.ctx());
        self.silence_window(ui.ctx());
        self.boring_window(ui.ctx());
        self.subtitle_windows(ui.ctx());
        self.title_tick();
        self.dynamic_tick(ui.ctx());
    }

    fn playhead_color(&self) -> Color32 {
        if self.timeline.dynamic.is_some() {
            Color32::from_rgb(240, 200, 40)
        } else {
            theme::colors().playhead
        }
    }

    pub(crate) fn lower_shows(&self, from: i64, to: i64) -> bool {
        let Some(l) = &self.timeline.layout else {
            return true;
        };
        let ppf = self.timeline.options.ppf;
        let visible = f64::from(l.lower_ruler.width()) / ppf;
        let first = if self.timeline.options.fixed_playhead {
            self.drawn_playhead() as f64 - visible / 2.0
        } else {
            self.timeline.scroll
        };
        (from as f64) < first + visible && (to as f64) > first
    }

    fn lower_scale(&mut self, content: Rect, playhead: f64) -> Scale {
        let ppf = self.timeline.options.ppf;
        if self.timeline.options.fixed_playhead {
            return Scale::anchored(content.center().x, playhead, ppf);
        }
        let visible = f64::from(content.width()) / ppf;
        let first = self.timeline.scroll;
        let p = playhead;
        if p < first || p > first + visible * 0.98 {
            self.timeline.scroll = (p - visible * 0.05).max(0.0);
        }
        Scale::anchored(content.left(), self.timeline.scroll, ppf)
    }

    fn timeline_toolbar(&mut self, ui: &mut egui::Ui, p: &Project) {
        use Toolbar as T;
        let row = crate::gui::row::Row {
            left: &[T::Title],
            fill: None,
            right: &[T::Marks, T::ZoomOut, T::ZoomIn, T::Summary],
            leaving: &[T::Marks, T::Summary, T::ZoomOut, T::ZoomIn],
            group: |t| match t {
                T::Title => 0,
                T::Marks | T::ZoomOut | T::ZoomIn => 1,
                T::Summary => 2,
            },
        };
        ui.horizontal(|ui| {
            self.fit_row(ui, row, |ws, ui, t| match t {
                T::Title => ui.label(RichText::new("Timeline").strong()),
                T::Summary => ui.label(dim(format!(
                    "{} clips, {}",
                    p.tracks.iter().map(|t| t.clips.len()).sum::<usize>(),
                    ws.tc(p.end())
                ))),
                T::ZoomIn => {
                    let r = ui.small_button("+");
                    if r.clicked() {
                        let o = &mut ws.timeline.options;
                        o.ppf = (o.ppf * 1.5).min(*ZOOM.end());
                        o.save();
                    }
                    r
                }
                T::ZoomOut => {
                    let r = ui.small_button("−");
                    if r.clicked() {
                        let o = &mut ws.timeline.options;
                        o.ppf = (o.ppf / 1.5).max(*ZOOM.start());
                        o.save();
                    }
                    r
                }
                T::Marks => {
                    let marks = match (p.marks.mark_in, p.marks.mark_out) {
                        (None, None) => "no In/Out".to_string(),
                        (a, b) => format!(
                            "In {}  Out {}",
                            a.map_or("-".into(), |f| ws.tc(f)),
                            b.map_or("-".into(), |f| ws.tc(f - 1)),
                        ),
                    };
                    ui.label(dim(marks))
                }
            });
        });
    }

    fn timeline_boxes(&mut self, ui: &mut egui::Ui, upper: Rect, lower: Rect, p: &Project) {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(upper.shrink(3.0)));
        child.set_clip_rect(upper.intersect(child.clip_rect()));
        child.spacing_mut().item_spacing = vec2(2.0, 2.0);
        child.spacing_mut().button_padding = vec2(3.0, 1.0);
        child.spacing_mut().interact_size.y = BOX_CONTROL;
        child.horizontal(|ui| {
            ui.menu_button(RichText::new("Options").small(), |ui| {
                self.options_menu(ui, p)
            });
            ui.menu_button(RichText::new("Actions").small(), |ui| {
                self.timeline_actions_menu(ui, p)
            });
            ui.menu_button(RichText::new("Edit").small(), |ui| {
                self.edit_actions_menu(ui)
            });
        });
        child.horizontal_wrapped(|ui| {
            for t in p.tracks.iter() {
                let on = self.is_target(&t.id);
                let r = ui.selectable_label(on, RichText::new(&t.id).small());
                let name = format!("Target {}", t.id);
                r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, &name));
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
        child.spacing_mut().interact_size.y = BOX_CONTROL;
        child.horizontal_centered(|ui| self.timeline_tools(ui, p));
    }

    fn timeline_tools(&mut self, ui: &mut egui::Ui, p: &Project) {
        let ripple = p.settings.ripple_on;
        if word_toggle(ui, "Ripple", "Ripple On", ripple).clicked() {
            self.send("project.set", json!({ "ripple_on": !ripple }));
        }
        let r = word_toggle(ui, "Trim", "Dynamic Trim", self.timeline.dynamic.is_some());
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
        if icon_toggle(ui, icon::SPLIT, "Split", false).clicked() {
            self.split_at_playhead();
        }
        if icon_toggle(ui, icon::MARKER, "Marker", false).clicked() {
            self.send("markers.add", json!({ "at": self.snap.timeline.position }));
        }
        if icon_toggle(ui, icon::SNAP, "Snap", self.snapping()).clicked() {
            self.timeline.options.snapping = !self.timeline.options.snapping;
            self.timeline.options.save();
        }
    }

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
        ui.checkbox(&mut o.minimize_subtitles, "Minimize Subtitle Track");
        ui.separator();
        ui.checkbox(&mut o.boring, "Boring Detector");
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

    pub(super) fn timeline_actions_menu(&mut self, ui: &mut egui::Ui, p: &Project) {
        if ui.button("Add Video Track").clicked() {
            self.send("track.add", json!({ "kind": "video" }));
            ui.close();
        }
        if ui.button("Add Audio Track").clicked() {
            self.send("track.add", json!({ "kind": "audio" }));
            ui.close();
        }
        if ui.button("Add Subtitle Track").clicked() {
            self.send("track.add", json!({ "kind": "subtitle" }));
            ui.close();
        }
        ui.separator();
        let at = self.snap.timeline.position;
        if ui.button(keys("Split Clips  (Ctrl+B)")).clicked() {
            self.split_at_playhead();
            ui.close();
        }
        if ui.button("Join Match Edit Clips").clicked() {
            self.send(
                "edit.join",
                json!({ "at": at, "clips": self.snap.selection.clips }),
            );
            ui.close();
        }
        if ui.button("Add Marker  (M)").clicked() {
            self.send("markers.add", json!({ "at": at }));
            ui.close();
        }
        if ui.button("Add Title").clicked() {
            self.add_title(ui.ctx());
            ui.close();
        }
        if ui.button("Add Subtitle").clicked() {
            self.add_subtitle(ui.ctx(), None);
            ui.close();
        }
        if ui.button("Create Subtitles from Audio…").clicked() {
            self.open_speech();
            ui.close();
        }
        let shown = vidcut_core::ops::subtitles::shown(p)
            .ok()
            .map(|i| &p.subtitles[i])
            .filter(|t| !t.cues.is_empty());
        if ui
            .add_enabled(
                shown.is_some(),
                egui::Button::new("Re-time Subtitles to Speech…"),
            )
            .clicked()
            && let Some(t) = shown
        {
            self.open_retime(&t.id);
            ui.close();
        }
        if shown.is_none() {
            super::dialog::why(ui, "the shown subtitle track has no subtitles");
        }
        ui.separator();
        if ui.button("Clear Transition from All Edits").clicked() {
            self.send("edit.remove_transition", json!({ "all": true }));
            ui.close();
        }
        if ui.button("Add Dissolve to All Edits").clicked() {
            self.send("edit.dissolve", json!({ "all": true }));
            ui.close();
        }
        ui.separator();
        let selected = self.snap.selection.clips.clone();
        let with = |want: fn(&vidcut_core::model::Clip) -> bool| {
            selected.iter().any(|id| p.clip(id).is_some_and(want))
        };
        let sound = with(|c| c.audio.is_some());
        if ui
            .add_enabled(sound, egui::Button::new("Ripple Delete Silence…"))
            .clicked()
        {
            self.open_silence(selected.clone());
            ui.close();
        }
        if !sound {
            super::dialog::why(ui, "select clips with sound");
        }
        let video = with(|c| c.video);
        if ui
            .add_enabled(video, egui::Button::new("Detect Scene Cuts"))
            .clicked()
        {
            self.send("edit.detect_scene_cuts", json!({ "clips": selected }));
            ui.close();
        }
        if !video {
            super::dialog::why(ui, "select clips with video");
        }
    }

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
        if ui
            .add_enabled(
                !self.snap.selection.clips.is_empty(),
                egui::Button::new(keys("Change Clip Speed…  (R)")),
            )
            .clicked()
        {
            self.open_speed_dialog();
            ui.close();
        }
        if self.snap.selection.clips.is_empty() {
            super::dialog::why(ui, "select clips first");
        }
    }

    fn scroll_rows(&mut self, ui: &egui::Ui, band: Rect, rows: &Rows) {
        let over = rows.overflow(NEW_TRACK_STRIP);
        if over <= 0.0 || self.timeline.drag.is_some() || !ui.rect_contains_pointer(band) {
            return;
        }
        let dy = ui.input(|i| match i.modifiers.command {
            true => 0.0,
            false => i.smooth_scroll_delta.y,
        });
        if dy != 0.0 {
            self.timeline.rows_lift = (self.timeline.rows_lift + dy).clamp(0.0, over);
            ui.ctx().request_repaint();
        }
    }

    fn row_scrollbar(&mut self, ui: &egui::Ui, rows: &Rows) {
        let over = rows.overflow(NEW_TRACK_STRIP);
        if over <= 0.0 {
            return;
        }
        let area = rows.area;
        let bar = Rect::from_min_max(
            pos2(area.right() - 8.0, area.top() + NEW_TRACK_STRIP),
            pos2(area.right() - 2.0, area.bottom()),
        );
        let used = rows.used().max(1.0);
        let h = bar.height();
        let thumb_h = (h * (used - over) / used).clamp(16.0_f32.min(h), h);
        let travel = (h - thumb_h).max(1.0);
        let resp = ui.interact(
            bar,
            ui.scope_id().with("row-scrollbar"),
            Sense::click_and_drag(),
        );
        if resp.dragged() {
            let dy = resp.drag_delta().y;
            self.timeline.rows_lift =
                (self.timeline.rows_lift - dy * over / travel).clamp(0.0, over);
        } else if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let at = ((pos.y - bar.top() - thumb_h / 2.0) / travel).clamp(0.0, 1.0);
            self.timeline.rows_lift = over * (1.0 - at);
        }
        let top = bar.top() + travel * (over - self.timeline.rows_lift) / over;
        let thumb = Rect::from_min_size(pos2(bar.left(), top), vec2(bar.width(), thumb_h));
        let c = theme::colors();
        let painter = ui.painter_at(bar);
        painter.rect_filled(bar, 3.0, c.panel_dark.gamma_multiply(0.8));
        let shade = if resp.hovered() || resp.dragged() {
            c.text_dim
        } else {
            c.line
        };
        painter.rect_filled(thumb, 3.0, shade);
    }

    fn track_headers(&mut self, ui: &mut egui::Ui, lower_rect: Rect, rows: &Rows, p: &Project) {
        for row in &rows.rows {
            let r = row.rect.rect;
            if !r.intersects(rows.area) {
                continue;
            }
            let rect = Rect::from_min_max(
                pos2(lower_rect.left() + 2.0, r.top()),
                pos2(lower_rect.left() + HEADER - 4.0, r.bottom()),
            );
            if let Some(s) = p.subtitle_track(&row.track) {
                self.subtitle_header(ui, rect, s);
                continue;
            }
            let Some(t) = p.track(&row.track) else {
                continue;
            };
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            child.set_clip_rect(rect.intersect(child.clip_rect()));
            child.spacing_mut().button_padding.x = 3.0;
            let is_target = self.is_target(&t.id);
            let label = t.name.clone().unwrap_or_else(|| t.id.clone());
            let parts = geometry::header_parts(rect);
            let mut name = child.new_child(
                egui::UiBuilder::new()
                    .max_rect(parts.name)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            let r = name.add(
                egui::Button::selectable(
                    is_target,
                    if is_target {
                        RichText::new(&label).small()
                    } else {
                        RichText::new(&label).strong().small()
                    },
                )
                .truncate(),
            );
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
                if ui.button("Add Subtitle Track").clicked() {
                    self.send("track.add", json!({ "kind": "subtitle" }));
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
            let enlarged = self.timeline.enlarged.as_deref() == Some(t.id.as_str());
            let glyph = if enlarged {
                icon::SHRINK
            } else {
                icon::ENLARGE
            };
            let name = format!("Enlarge {}", t.id);
            if header_button(&mut child, parts.enlarge, glyph, &name, enlarged).clicked() {
                self.timeline.enlarged = (!enlarged).then(|| t.id.clone());
            }
            let lock = if t.locked {
                icon::LOCKED
            } else {
                icon::UNLOCKED
            };
            let sound = if t.muted {
                icon::AUDIO_OFF
            } else {
                icon::AUDIO_ON
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
                    icon::VIDEO_ON
                } else {
                    icon::VIDEO_OFF
                };
                let change = json!({ "track": t.id, "video_enabled": !t.video_enabled });
                buttons.push(("Video", !t.video_enabled, picture, change));
            }
            for ((what, lit, glyph, change), at) in buttons.into_iter().zip(parts.toggles) {
                let name = format!("{what} {}", t.id);
                if header_button(&mut child, at, glyph, &name, lit).clicked() {
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
        selected: &'a [String],
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
            selected,
        }
    }

    fn start_drag(&mut self, ui: &egui::Ui, p: &Project, upper: bool, layout: &Layout) {
        let Some(press) = ui.input(|i| i.pointer.press_origin()) else {
            return;
        };
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
        let playhead = self.drawn_playhead();
        let (track, what) = hit_rows(p, rows, &scale, press);
        let gesture = if ruler.expand2(vec2(0.0, 2.0)).contains(press) || track.is_none() && !upper
        {
            mark_grab(p, ruler, &scale, press).unwrap_or(Gesture::Scrub {
                drag_content: fixed,
                from: playhead as f64,
            })
        } else if let Some(g) = self.cue_gesture(p, rows, &scale, press, upper, mods.command) {
            g
        } else if let Some((track, at, end, x, sound)) = rows.at(press.y).and_then(|r| {
            let t = p.track(&r.track)?;
            let (at, end) = geometry::fade_edge_at(p, t, &scale, &r.rect, press)?;
            let x = geometry::picture_transition(p, t, at)?;
            let sound = t
                .transitions
                .iter()
                .any(|y| y.sound && y.from == x.from && y.to == x.to);
            Some((r.track.clone(), at, end, x.clone(), sound))
        }) {
            if let Some(t) = TrackRef::of(&track) {
                self.select_transition(t, at);
            }
            Gesture::Fade {
                track,
                at,
                end,
                before: x.before,
                after: x.after,
                sound,
            }
        } else {
            let track = track.unwrap_or_else(|| MAIN.to_string());
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
                        self.send("selection.set", json!({ "clips": [id], "cues": [] }));
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
        if vidcut_av::debug() {
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
        if let Gesture::Bin { path } = &d.drag.gesture {
            let path = path.clone();
            if released {
                if let Some(to) = self.bin_rename_to(&path, pos) {
                    self.send("bins.rename", json!({ "path": path, "to": to }));
                }
                self.timeline.snap_flipped = false;
            } else {
                self.timeline.drag = Some(d);
                repaint_in(ui.ctx(), HELD_FRAME);
            }
            return;
        }
        if released
            && let Gesture::Media { media, .. } | Gesture::Subtitles { media, .. } = &d.drag.gesture
            && let Some(bin) = self.bin_target_at(pos)
        {
            let ids = if self.snap.selection.media.contains(media) {
                self.snap.selection.media.clone()
            } else {
                vec![media.clone()]
            };
            self.send("media.move", json!({ "media": ids, "bin": bin }));
            self.timeline.snap_flipped = false;
            return;
        }
        if matches!(
            d.drag.gesture,
            Gesture::Media { .. } | Gesture::Title { .. } | Gesture::Subtitles { .. }
        ) {
            d.upper = layout.upper.area.expand(8.0).contains(pos);
        }
        let moving: Vec<String> = match &d.drag.gesture {
            Gesture::Move { clips, .. } => clips.clone(),
            Gesture::CueMove { cues, .. } => cues.clone(),
            Gesture::CueTrim { cue, .. } => vec![cue.clone()],
            _ => Vec::new(),
        };
        let scrubbing = matches!(d.drag.gesture, Gesture::Scrub { .. });
        let fixed = !d.upper && self.timeline.options.fixed_playhead;
        let playhead = (!fixed && !scrubbing).then_some(self.snap.timeline.position);
        let snap = Snap::new(p, playhead, &moving);
        let picked: Vec<String> = self
            .snap
            .selection
            .clips
            .iter()
            .chain(&self.snap.selection.cues)
            .cloned()
            .collect();
        let (preview, request) = {
            let ctx = self.ctx_for(p, d.upper, layout, Some(&snap), &picked);
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
        repaint_in(ui.ctx(), HELD_FRAME);
    }

    fn wheel(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        p: &Project,
        upper: bool,
        layout: &Layout,
        now: Instant,
    ) {
        if !resp.hovered() || self.timeline.drag.is_some() {
            return;
        }
        let (zoom, scroll) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta));
        if !upper && zoom != 1.0 {
            let o = &mut self.timeline.options;
            o.ppf = (o.ppf * f64::from(zoom)).clamp(*ZOOM.start(), *ZOOM.end());
            o.save();
            return;
        }
        if scroll.x == 0.0 {
            return;
        }
        if !upper && !self.timeline.options.fixed_playhead {
            self.timeline.scroll =
                (self.timeline.scroll - f64::from(scroll.x) / layout.lower_scale.ppf).max(0.0);
            return;
        }
        let ppf = if upper {
            layout.upper_scale.ppf
        } else {
            layout.lower_scale.ppf
        };
        let from = self
            .timeline
            .swipe
            .map_or(self.drawn_playhead() as f64, |(f, _)| f);
        let to = swipe(from, scroll.x, ppf, !upper, p.end());
        if to.round() != from.round() {
            self.seek(PlayerId::Timeline, to.round() as i64, true);
        }
        self.timeline.swipe = Some((to, now));
    }

    fn end_swipe(&mut self, ctx: &egui::Context, now: Instant) {
        const QUIET: Duration = Duration::from_millis(300);
        let Some((frame, last)) = self.timeline.swipe else {
            return;
        };
        if now.duration_since(last) < QUIET {
            ctx.request_repaint_after(QUIET);
            return;
        }
        self.timeline.swipe = None;
        self.seek(PlayerId::Timeline, frame.round() as i64, false);
    }

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
        if resp.secondary_clicked()
            && let Some(pos) = resp.interact_pointer_pos()
            && !self.cue_secondary(p, rows, &scale, pos, upper)
        {
            self.timeline.cue_menu = None;
            let faded = rows.at(pos.y).and_then(|r| {
                let at = geometry::transition_at(p, p.track(&r.track)?, &scale, &r.rect, pos)?;
                Some((TrackRef::of(&r.track)?, at))
            });
            if let Some((track, at)) = faded {
                self.select_transition(track, at);
                self.timeline.transition_menu = Some(upper);
                self.timeline.clip_menu = None;
            } else {
                self.timeline.transition_menu = None;
                let id = match hit_rows(p, rows, &scale, pos).1 {
                    Hit::Clip(id) | Hit::Slip(id) | Hit::Edge(id, _) => Some(id),
                    _ => None,
                };
                if let Some(id) = &id
                    && !self.snap.selection.clips.contains(id)
                {
                    self.send("selection.set", json!({ "clips": [id], "cues": [] }));
                }
                self.timeline.clip_menu = id.map(|id| (upper, id));
            }
        }
        if self.timeline.transition_menu == Some(upper)
            && resp.context_menu(|ui| self.transition_menu(ui)).is_none()
        {
            self.timeline.transition_menu = None;
        }
        if let Some((on_upper, id)) = self.timeline.clip_menu.clone()
            && on_upper == upper
        {
            let shown = p
                .clip(&id)
                .cloned()
                .and_then(|c| resp.context_menu(|ui| self.clip_menu(ui, &c)));
            if shown.is_none() {
                self.timeline.clip_menu = None;
            }
        }
        self.cue_menus(resp, p, upper);
        let Some(pos) = resp.interact_pointer_pos() else {
            return;
        };
        let mods = ui.input(|i| i.modifiers);
        let on_ruler = ruler.expand2(vec2(0.0, 2.0)).contains(pos);
        if !on_ruler && self.cue_click(resp, p, (rows, &scale), pos, upper, mods) {
            return;
        }
        let what = hit_rows(p, rows, &scale, pos).1;
        let faded = rows
            .at(pos.y)
            .filter(|_| !on_ruler && !matches!(what, Hit::Roll { .. } | Hit::AudioEdit { .. }))
            .and_then(|r| {
                let at = geometry::transition_at(p, p.track(&r.track)?, &scale, &r.rect, pos)?;
                Some((TrackRef::of(&r.track)?, at))
            });
        if resp.double_clicked() || resp.triple_clicked() {
            if on_ruler {
                if let Some(k) = p
                    .markers
                    .iter()
                    .find(|k| (scale.xf(k.at) - pos.x).abs() <= 5.0)
                {
                    self.open_marker(k.id.clone(), p);
                }
            } else if let Some((track, at)) = faded {
                self.select_transition(track, at);
            } else if let Hit::Clip(id) | Hit::Slip(id) = &what
                && p.clip(id).is_some_and(|c| c.title.is_some())
            {
                self.open_title(id);
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
            self.timeline.transition = None;
            if let Some((track, at)) = faded {
                self.select_transition(track, at);
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
                        self.send("selection.set", json!({ "clips": [], "cues": [] }));
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
        let params = if mods.command {
            json!({ "clips": sel })
        } else {
            json!({ "clips": sel, "cues": [] })
        };
        self.send("selection.set", params);
    }

    fn clip_menu(&mut self, ui: &mut egui::Ui, c: &vidcut_core::model::Clip) {
        let clips = if self.snap.selection.clips.contains(&c.id) {
            self.snap.selection.clips.clone()
        } else {
            vec![c.id.clone()]
        };
        if c.title.is_some() && ui.button("Edit Title…").clicked() {
            self.open_title(&c.id);
            ui.close();
        }
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
        let current = self.project().and_then(|p| {
            let shown = clips
                .iter()
                .map(|id| p.clip(id).and_then(|c| p.clip_color(c)));
            theme::same_color(shown).map(str::to_string)
        });
        ui.menu_button("Clip Color", |ui| {
            if let Some(color) = theme::clip_color_menu(ui, current.as_deref()) {
                self.send("clip.set", json!({ "clips": clips, "color": color }));
                ui.close();
            }
        });
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
        if c.compound.is_some() {
            ui.separator();
            item(
                ui,
                "Decompose in Place",
                "edit.decompose",
                json!({ "clips": [c.id] }),
            );
        }
        if vidcut_core::ops::picture::has_picture(c) {
            ui.separator();
            let at = self.snap.player(crate::live::PlayerId::Timeline).position;
            for (label, tool) in [
                ("Transform  (T)", PictureTool::Transform),
                ("Crop  (Shift+T)", PictureTool::Crop),
                ("Dynamic Zoom  (Alt+T)", PictureTool::DynamicZoom),
            ] {
                if ui.button(keys(label)).clicked() {
                    self.send("selection.set", json!({ "clips": [c.id] }));
                    if !(c.start..c.end()).contains(&at) {
                        self.send("player.seek", json!({ "at": c.start }));
                    }
                    self.timeline.picture_grab.tool = Some(tool);
                    ui.close();
                }
            }
            for (label, tool) in [
                ("Reset Transform", "transform"),
                ("Reset Crop", "crop"),
                ("Reset Dynamic Zoom", "dynamic_zoom"),
            ] {
                if ui.button(label).clicked() {
                    self.send("picture.clear", json!({ "clips": clips, "tool": tool }));
                    ui.close();
                }
            }
        }
        ui.separator();
        if ui.button(keys("Change Duration…  (Ctrl+D)")).clicked() {
            self.timeline.duration_dialog = Some(String::new());
            ui.close();
        }
        if !c.is_generator() && ui.button(keys("Change Speed…  (R)")).clicked() {
            self.open_speed_dialog();
            ui.close();
        }
        if c.audio.is_some() {
            let heard = self
                .project()
                .and_then(|p| super::isolation::heard(p, &c.id))
                .flatten();
            let on = super::isolation::Isolate::Clips(clips.clone());
            let label = keys("Voice Isolation  (Alt+V)");
            if self.isolation_controls(ui, on, heard, &label) {
                ui.close();
            }
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
            egui::Id::unique("timeline-drag-tip"),
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

    pub(crate) fn start_subtitle_drag(&mut self, media: &str, press: Pos2) {
        let Some(p) = self.project_shared() else {
            return;
        };
        let Some(m) = p.media(media) else { return };
        let read = std::fs::read(&m.path)
            .map_err(|e| e.to_string())
            .and_then(|b| vidcut_core::subtitles::parse(&b, &m.path).map_err(|e| e.message));
        let read = match read {
            Ok((read, ..)) => read,
            Err(e) => {
                self.say(format!("{}: {e}", m.name));
                return;
            }
        };
        let first = read
            .cues
            .iter()
            .map(|c| c.start)
            .fold(f64::INFINITY, f64::min);
        let fps = p.settings.fps.as_f64();
        let frame = |t: f64| ((t - first) * fps).round() as i64;
        let cues: Vec<(i64, i64)> = read
            .cues
            .iter()
            .map(|c| (frame(c.start), frame(c.end).max(frame(c.start) + 1)))
            .collect();
        if cues.is_empty() {
            return;
        }
        self.timeline.snap_flipped = false;
        self.timeline.drag = Some(ActiveDrag {
            drag: Drag::new(
                Gesture::Subtitles {
                    media: media.to_string(),
                    cues,
                },
                press,
            ),
            upper: false,
            preview: Preview::default(),
        });
    }

    pub(crate) fn start_title_drag(
        &mut self,
        p: &Project,
        template: crate::gui::title_browser::Template,
        press: Pos2,
    ) {
        self.timeline.snap_flipped = false;
        self.timeline.drag = Some(ActiveDrag {
            drag: Drag::new(
                Gesture::Title {
                    template: Box::new(template),
                    len: vidcut_core::ops::titles::default_length(p),
                },
                press,
            ),
            upper: false,
            preview: Preview::default(),
        });
    }

    pub(crate) fn media_drag_overlay(&self, ctx: &egui::Context) {
        if let Some(text) = self.bin_drag_label()
            && let Some(pos) = ctx.input(|i| i.pointer.latest_pos())
        {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                egui::Id::unique("bin-drag"),
            ));
            let galley = painter.layout_no_wrap(text, FontId::proportional(12.0), Color32::WHITE);
            let at = pos + vec2(12.0, 10.0);
            painter.rect_filled(
                Rect::from_min_size(at - vec2(4.0, 2.0), galley.size() + vec2(8.0, 4.0)),
                3.0,
                theme::colors().clip,
            );
            painter.galley(at, galley, Color32::WHITE);
            return;
        }
        let Some(d) = &self.timeline.drag else { return };
        let (name, range) = match &d.drag.gesture {
            Gesture::Media { media, range, .. } => (
                self.project()
                    .and_then(|p| p.media(media))
                    .map_or(media.clone(), |m| m.name.clone()),
                *range,
            ),
            Gesture::Subtitles { media, cues } => (
                self.project()
                    .and_then(|p| p.media(media))
                    .map_or(media.clone(), |m| m.name.clone()),
                (0, cues.iter().map(|c| c.1).max().unwrap_or(0)),
            ),
            Gesture::Title { template, len } => (template.label.clone(), (0, *len)),
            _ => return,
        };
        let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) else {
            return;
        };
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::unique("media-drag"),
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
            theme::colors().clip,
        );
        painter.galley(at, galley, Color32::WHITE);
        if d.preview.request.is_none() {
            painter.text(
                at + vec2(0.0, 20.0),
                Align2::LEFT_TOP,
                "drop on the timeline",
                FontId::proportional(10.0),
                theme::colors().text_dim,
            );
        }
    }

    pub(crate) fn timeline_keys(&mut self, input: &egui::InputState) {
        let Some(p) = self.project_shared() else {
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
            self.timeline.transition = None;
        }
        if pressed(Key::U)
            && let Some(e) = &mut self.timeline.edit
        {
            e.side = match e.side {
                Side::Both => Side::In,
                Side::In => Side::Out,
                Side::Out => Side::Both,
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
        if nudge != 0 && self.timeline.edit.is_some() {
            if self
                .timeline
                .dynamic
                .as_ref()
                .is_some_and(tools::Dynamic::running)
            {
                self.dynamic_stop();
            }
            if let Some(e) = self.timeline.edit {
                self.nudge_edit(e.side, nudge);
            }
        }
        if pressed(Key::D) && input.modifiers.command {
            if !self.snap.selection.clips.is_empty() {
                self.timeline.duration_dialog = Some(String::new());
            } else if self.selected_transition().is_some() {
                self.open_fade_dialog();
            }
        }
        if pressed(Key::R) && !input.modifiers.command && !self.snap.selection.clips.is_empty() {
            self.open_speed_dialog();
        }
        if pressed(Key::W) && !input.modifiers.command {
            self.toggle_dynamic();
        }
        self.silence_keys(input);
    }

    pub(crate) fn nudge_edit(&mut self, side: Side, by: i64) {
        let Some(e) = self.timeline.edit else { return };
        let carried = self.timeline.transition == Some((e.track, e.at));
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
            let mut q = Project::clone(&p);
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
        let mut q = Project::clone(&p);
        let tried = match (method, &params["edge"]) {
            ("edit.roll", _) => ops::roll(&mut q, &id, Edge::Tail, by),
            (_, edge) if edge == "head" => ops::trim(&mut q, &id, Edge::Head, by, None),
            _ => ops::trim(&mut q, &id, Edge::Tail, by, None),
        };
        self.send(method, params);
        if tried.is_err() {
            return;
        }
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
        if carried {
            self.timeline.transition = Some((e.track, at));
        }
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
        super::dialog::window(ctx, "Marker", &mut open, |ui| {
            super::dialog::form(ui, "marker-form", |ui| {
                super::dialog::row(ui, "Name", |ui| {
                    ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(260.0));
                });
                super::dialog::row(ui, "Notes", |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut d.note)
                            .desired_rows(3)
                            .desired_width(260.0),
                    );
                });
                super::dialog::row(ui, "Color", |ui| {
                    egui::ComboBox::from_id_salt("marker-color")
                        .selected_text(&d.color)
                        .show_ui(ui, |ui| {
                            for c in ops::MARKER_COLORS {
                                ui.selectable_value(&mut d.color, c.to_string(), *c);
                            }
                        });
                });
            });
            super::dialog::footer(ui, |ui| {
                if super::dialog::main_button(ui, "Done", true) {
                    self.send(
                        "markers.set",
                        json!({ "marker": d.id, "name": d.name, "note": d.note, "color": d.color }),
                    );
                    done = true;
                }
                if ui.button("Cancel").clicked() {
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

    fn duration_window(&mut self, ctx: &egui::Context) {
        let Some(mut text) = self.timeline.duration_dialog.take() else {
            return;
        };
        let clips = self.snap.selection.clips.clone();
        let mut open = !clips.is_empty();
        let mut done = false;
        super::dialog::window(ctx, "Change Clip Duration", &mut open, |ui| {
            let set = |d: Value| {
                self.send(
                    "edit.change_duration",
                    json!({ "clips": clips, "duration": d }),
                );
            };
            let typed = crate::gui::viewer::timecode_input(&text, self.dropped());
            let mut enter = false;
            super::dialog::form(ui, "duration-form", |ui| {
                super::dialog::row(ui, "Clips", |ui| ui.label(clips.len().to_string()));
                super::dialog::row(ui, "Duration", |ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut text)
                            .hint_text("90f, 3s or a timecode")
                            .desired_width(200.0),
                    );
                    r.request_focus();
                    enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                });
                super::dialog::row(ui, "Or", |ui| {
                    for s in [1, 5, 15] {
                        if ui.button(format!("{s} s")).clicked() {
                            set(json!(format!("{s}s")));
                            done = true;
                        }
                    }
                    if ui.button("To the End").clicked() {
                        set(json!("end"));
                        done = true;
                    }
                });
            });
            super::dialog::footer(ui, |ui| {
                let change = super::dialog::main_button(ui, "Change", typed.is_some());
                if (change || enter)
                    && let Some(spec) = &typed
                {
                    set(json!(spec));
                    done = true;
                }
                if ui.button("Cancel").clicked() {
                    done = true;
                }
                if typed.is_none() && !text.trim().is_empty() {
                    super::dialog::why(ui, "not a duration");
                }
            });
        });
        if open && !done {
            self.timeline.duration_dialog = Some(text);
        }
    }

    fn open_speed_dialog(&mut self) {
        let Some(p) = self.project() else {
            return;
        };
        let speed: f64 = self
            .snap
            .selection
            .clips
            .first()
            .and_then(|id| p.clip(id))
            .map_or(1.0, |c| c.speed());
        let ripple = p.settings.ripple_on;
        self.timeline.speed_dialog = Some(SpeedDialog {
            percent: if speed == 0.0 {
                100.0
            } else {
                speed.abs() * 100.0
            },
            reverse: speed < 0.0,
            freeze: speed == 0.0,
            ripple,
        });
    }

    fn speed_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.timeline.speed_dialog.take() else {
            return;
        };
        let clips = self.snap.selection.clips.clone();
        let mut open = !clips.is_empty();
        let mut done = false;
        super::dialog::window(ctx, "Change Clip Speed", &mut open, |ui| {
            super::dialog::form(ui, "speed-form", |ui| {
                super::dialog::row(ui, "Clips", |ui| ui.label(clips.len().to_string()));
                super::dialog::row(ui, "Speed", |ui| {
                    ui.add_enabled(
                        !d.freeze,
                        egui::DragValue::new(&mut d.percent)
                            .range(1.0..=10000.0)
                            .speed(1.0)
                            .suffix(" %"),
                    );
                    ui.add_enabled(!d.freeze, egui::Checkbox::new(&mut d.reverse, "Reverse"));
                    if d.freeze {
                        super::dialog::why(ui, "a freeze frame holds one picture");
                    }
                });
                super::dialog::row(ui, "Freeze Frame", |ui| ui.checkbox(&mut d.freeze, ""));
                super::dialog::row(ui, "Ripple Timeline", |ui| ui.checkbox(&mut d.ripple, ""));
            });
            super::dialog::footer(ui, |ui| {
                let change = super::dialog::main_button(ui, "Change", true)
                    || ui.input(|i| i.key_pressed(Key::Enter));
                if change {
                    let speed = if d.freeze {
                        0.0
                    } else if d.reverse {
                        -d.percent / 100.0
                    } else {
                        d.percent / 100.0
                    };
                    self.send(
                        "edit.change_speed",
                        json!({ "clips": clips, "speed": speed, "ripple": d.ripple }),
                    );
                    done = true;
                }
                if ui.button("Cancel").clicked() {
                    done = true;
                }
            });
        });
        if open && !done {
            self.timeline.speed_dialog = Some(d);
        }
    }

    pub(crate) fn open_trim_editor(&mut self, at: i64, track: TrackRef) {
        self.timeline.edit = Some(EditSel {
            at,
            side: Side::Both,
            track,
        });
        self.timeline.transition = None;
        self.show_timeline();
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_edit_selection(
        &self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        rows: &Rows,
        scale: &Scale,
        p: &Project,
        moved: Option<i64>,
        reach: f32,
    ) {
        let Some(e) = self.timeline.edit else {
            return;
        };
        let Some(row) = rows.get(&e.track.id()) else {
            return;
        };
        let x = scale.xf(moved.unwrap_or(e.at));
        let y = if e.side == Side::Audio {
            egui::Rangef::new(row.rect.audio_top, row.rect.rect.bottom())
        } else {
            row.rect.rect.y_range()
        };
        let stopped = self.dynamic_stopped().is_some();
        let (out_ok, in_ok) = self.cut_media(p, moved);
        let glow = paint::pulse(ctx.input(|i| i.time));
        let tint = |ok: bool| {
            if stopped || !ok {
                (
                    Color32::from_rgb(200, 40, 40),
                    Color32::from_rgb(240, 70, 70),
                )
            } else {
                (
                    Color32::from_rgb(20, 150, 20),
                    Color32::from_rgb(60, 230, 60),
                )
            }
        };
        let shade = |side: f32, ok: bool| {
            let (_, light) = tint(ok);
            let steps = reach.round().max(1.0) as usize;
            for i in 0..steps {
                let fade = 1.0 - i as f32 / steps as f32;
                let a = x + side * (1.0 + i as f32);
                let b = a + side;
                painter.rect_filled(
                    Rect::from_x_y_ranges(a.min(b)..=a.max(b), y),
                    0.0,
                    light.gamma_multiply(glow * fade),
                );
            }
        };
        let (out_side, in_side) = match e.side {
            Side::Out => (true, false),
            Side::In => (false, true),
            Side::Both | Side::Audio => (true, true),
        };
        if out_side {
            shade(-1.0, out_ok);
        }
        if in_side {
            shade(1.0, in_ok);
        }
        let (core, _) = tint(if e.side == Side::In { in_ok } else { out_ok });
        painter.rect_filled(Rect::from_x_y_ranges((x - 1.0)..=(x + 1.0), y), 0.0, core);
        repaint_in(ctx, paint::PULSE_STEP);
    }

    pub(crate) fn select_transition(&mut self, track: TrackRef, at: i64) {
        self.open_trim_editor(at, track);
        self.timeline.transition = Some((track, at));
        if !self.snap.selection.clips.is_empty() {
            self.send("selection.set", json!({ "clips": [] }));
        }
    }

    pub(crate) fn trim_transition(
        &self,
        p: &Project,
        e: EditSel,
    ) -> Option<vidcut_core::model::Transition> {
        geometry::picture_transition(p, p.track(&e.track.id())?, e.at).cloned()
    }

    fn cut_media(&self, p: &Project, moved: Option<i64>) -> (bool, bool) {
        let Some(e) = self.timeline.edit else {
            return (true, true);
        };
        match (e.side, p.track(&e.track.id())) {
            (Side::Audio, _) | (_, None) => (true, true),
            (_, Some(t)) => ops::filmstrip::extends(p, t, moved.unwrap_or(e.at)),
        }
    }

    pub fn drawn_cut_media(&mut self) -> Option<(bool, bool)> {
        self.timeline.edit?;
        let p = self.shown_project()?;
        Some(match self.trim_drawn(&p) {
            Some((q, at)) => self.cut_media(&q, Some(at)),
            None => self.cut_media(&p, None),
        })
    }

    pub(crate) fn drawn_playhead(&self) -> i64 {
        let position = self.snap.timeline.position;
        self.timeline
            .landing
            .and_then(|(at, since, from)| tools::landing(at, since.elapsed(), from, position))
            .unwrap_or(position)
    }

    pub(crate) fn glided_playhead(&self, at: i64) -> f64 {
        let s = &self.snap.timeline;
        match s.glide {
            Some(g) if s.playing && at == s.position => {
                let ahead = g.fraction + g.at.elapsed().as_secs_f64() * g.rate;
                at as f64 + ahead.clamp(0.0, 1.0)
            }
            _ => at as f64,
        }
    }

    fn glide_repaint(&self, ctx: &egui::Context, ppf: f64) {
        let s = &self.snap.timeline;
        let Some(g) = s.glide.filter(|_| s.playing) else {
            return;
        };
        let shown = ctx.input(|i| {
            let v = i.viewport();
            v.minimized != Some(true) && v.occluded != Some(true)
        });
        let pixels_per_second = ppf * g.rate.abs() * f64::from(ctx.pixels_per_point());
        if shown && pixels_per_second > 0.0 {
            ctx.request_repaint_after(Duration::from_secs_f64(1.0 / pixels_per_second));
        }
    }

    fn landed_playhead(&mut self) -> i64 {
        let drawn = self.drawn_playhead();
        if drawn == self.snap.timeline.position {
            self.timeline.landing = None;
        }
        drawn
    }

    pub(crate) fn trim_editor_edit(&self) -> Option<EditSel> {
        let e = self
            .timeline
            .edit
            .filter(|e| e.side != Side::Audio && !e.track.audio)?;
        let running = self
            .timeline
            .dynamic
            .as_ref()
            .is_some_and(tools::Dynamic::running);
        let stopped = !self.snap.timeline.playing && !running;
        (stopped && self.snap.viewer == ViewerMode::Timeline).then_some(e)
    }

    pub fn trim_editor_request(&mut self, r: crate::live::TrimEditor) {
        let crate::live::TrimEditor::Open {
            at,
            track,
            by,
            side,
            transition,
        } = r
        else {
            self.timeline.edit = None;
            return;
        };
        let (Some(t), Some(p)) = (TrackRef::of(&track), self.shown_project()) else {
            return;
        };
        let nearest = |at: i64| {
            p.track(&track).and_then(|tr| {
                tr.clips
                    .iter()
                    .map(|c| c.end())
                    .min_by_key(|&e| ((e - at).abs(), e))
            })
        };
        let shown = self
            .timeline
            .edit
            .filter(|e| e.side != Side::Audio && e.track == t);
        let edit = match (at, shown) {
            (Some(at), _) => nearest(at),
            (None, Some(e)) => Some(e.at),
            (None, None) => nearest(self.snap.timeline.position),
        };
        let Some(edit) = edit else { return };
        if shown.map(|e| e.at) != Some(edit) {
            self.open_trim_editor(edit, t);
        }
        self.timeline.transition = transition.then_some((t, edit));
        if let Some(by) = by {
            self.timeline.edit = Some(EditSel {
                at: edit,
                side: Side::Both,
                track: t,
            });
            if self.set_edit_side(side.into()).is_ok() {
                self.nudge_edit(side.into(), by);
            }
        }
    }

    pub fn select_edit(&mut self, near: i64, track: &str, side: Side) -> Result<(), String> {
        let t = TrackRef::of(track).ok_or_else(|| format!("There is no {track}"))?;
        let p = self.shown_project().ok_or("No project is open")?;
        let tr = p
            .track(track)
            .ok_or_else(|| format!("There is no {track}"))?;
        let at = nearest_edit(tr, near, side).ok_or_else(|| match side {
            Side::Audio => format!("{track} has no cut between two clips with sound"),
            _ => format!("{track} has no clips"),
        })?;
        self.timeline.edit = Some(EditSel { at, side, track: t });
        self.timeline.transition = None;
        Ok(())
    }

    pub fn set_edit_side(&mut self, side: Side) -> Result<(), String> {
        let e = self.timeline.edit.ok_or("No edit is selected")?;
        let p = self.shown_project().ok_or("No project is open")?;
        let t = p
            .track(&e.track.id())
            .ok_or_else(|| format!("There is no {}", e.track.id()))?;
        let at = match (e.side, side) {
            (Side::Audio, Side::Audio) => e.at,
            (Side::Audio, _) => audio_edits(t)
                .find(|&(_, at)| at == e.at)
                .map_or(e.at, |(cut, _)| cut),
            (_, Side::Audio) => audio_edits(t)
                .find(|&(cut, _)| cut == e.at)
                .map(|(_, at)| at)
                .ok_or("No audio edit here: one clip has no sound")?,
            _ => e.at,
        };
        self.timeline.edit = Some(EditSel {
            at,
            side,
            track: e.track,
        });
        Ok(())
    }

    pub fn set_timeline_options(
        &mut self,
        set: &vidcut_core::request::TimelineOptions,
    ) -> Result<(), String> {
        if set.zoom.is_some_and(|z| !ZOOM.contains(&z)) {
            return Err("The zoom goes from 0.01 to 40 pixels a frame".into());
        }
        if set.boring_secs.is_some_and(|s| !BORING_SECS.contains(&s)) {
            return Err("Boring Clips goes from 1 to 300 seconds".into());
        }
        if set.jump_frames.is_some_and(|f| !JUMP_FRAMES.contains(&f)) {
            return Err("Jump Cuts goes from 1 to 120 frames".into());
        }
        let o = &mut self.timeline.options;
        let before = *o;
        o.ppf = set.zoom.unwrap_or(o.ppf);
        o.fixed_playhead = set.fixed_playhead.unwrap_or(o.fixed_playhead);
        o.snapping = set.snap.unwrap_or(o.snapping);
        o.safe_edit = set.safe_edit.unwrap_or(o.safe_edit);
        o.names = set.names.unwrap_or(o.names);
        o.durations = set.durations.unwrap_or(o.durations);
        o.status = set.status.unwrap_or(o.status);
        o.filmstrips = set.filmstrips.unwrap_or(o.filmstrips);
        o.waveforms = set.waveforms.unwrap_or(o.waveforms);
        o.boring = set.boring.unwrap_or(o.boring);
        o.boring_secs = set.boring_secs.unwrap_or(o.boring_secs);
        o.jump_frames = set.jump_frames.unwrap_or(o.jump_frames);
        o.minimize_subtitles = set.minimize_subtitles.unwrap_or(o.minimize_subtitles);
        if *o != before {
            o.save();
        }
        Ok(())
    }

    pub(crate) fn timeline_state(&self) -> crate::live::TimelineState {
        let o = &self.timeline.options;
        crate::live::TimelineState {
            zoom: o.ppf,
            fixed_playhead: o.fixed_playhead,
            snap: o.snapping,
            safe_edit: o.safe_edit,
            names: o.names,
            durations: o.durations,
            status: o.status,
            filmstrips: o.filmstrips,
            waveforms: o.waveforms,
            boring: o.boring,
            boring_secs: o.boring_secs,
            jump_frames: o.jump_frames,
            minimize_subtitles: o.minimize_subtitles,
        }
    }

    pub(crate) fn edit_point(&self) -> Option<crate::live::EditPoint> {
        self.timeline.edit.map(|e| crate::live::EditPoint {
            at: e.at,
            track: e.track.id(),
            side: e.side.into(),
        })
    }
}

impl From<vidcut_core::request::TrimSide> for Side {
    fn from(s: vidcut_core::request::TrimSide) -> Self {
        use vidcut_core::request::TrimSide;
        match s {
            TrimSide::Out => Self::Out,
            TrimSide::In => Self::In,
            TrimSide::Both => Self::Both,
            TrimSide::Audio => Self::Audio,
        }
    }
}

impl From<Side> for vidcut_core::request::TrimSide {
    fn from(s: Side) -> Self {
        match s {
            Side::Out => Self::Out,
            Side::In => Self::In,
            Side::Both => Self::Both,
            Side::Audio => Self::Audio,
        }
    }
}

fn mark_grab(p: &Project, ruler: Rect, scale: &Scale, pos: Pos2) -> Option<Gesture> {
    let band = Rect::from_x_y_ranges(
        ruler.x_range(),
        ruler.bottom() - MARK_BAND..=ruler.bottom() + 2.0,
    );
    if !band.contains(pos) {
        return None;
    }
    let near = |f: i64| (scale.xf(f) - pos.x).abs() <= 5.0;
    if let Some(k) = p.markers.iter().find(|k| near(k.at)) {
        return Some(Gesture::Marker {
            id: k.id.clone(),
            from: k.at,
        });
    }
    let (is_in, from) = match (p.marks.mark_in, p.marks.mark_out) {
        (Some(i), _) if near(i) => (true, i),
        (_, Some(o)) if near(o) => (false, o),
        _ => return None,
    };
    Some(Gesture::Mark { is_in, from })
}

fn focus(m: &crate::gui::history::Marks) -> Option<(i64, i64)> {
    let from = m.spans.iter().map(|s| s.from).min()?;
    let to = m.spans.iter().map(|s| s.to).max()?;
    Some((from, to))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Toolbar {
    Title,
    Marks,
    ZoomOut,
    ZoomIn,
    Summary,
}

#[cfg(test)]
mod tests {
    use vidcut_core::model::{Clip, ClipAudio, TrackKind};

    use super::*;

    #[test]
    fn a_repaint_in_16_ms_comes_in_16_ms() {
        let ctx = egui::Context::default();
        let pass = || {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                repaint_in(ui.ctx(), Duration::from_millis(16));
            });
            out.textures_delta.clear();
            out.viewport_output[&egui::ViewportId::ROOT].repaint_delay
        };
        let delays: Vec<Duration> = (0..3).map(|_| pass()).collect();
        assert_eq!(delays[2], Duration::from_millis(16), "{delays:?}");
    }

    fn clip(id: &str, start: i64, duration: i64, tail: Option<i64>) -> Clip {
        Clip {
            id: id.into(),
            media: "m1".into(),
            start,
            duration,
            source_in: 0,
            video: true,
            audio: tail.map(|tail| ClipAudio {
                stream: 0,
                head: 0,
                tail,
                gain_db: 0.0,
                muted: false,
                effects: None,
                voice_isolation: None,
            }),
            enabled: true,
            name: None,
            effects: None,
            generator: None,
            title: None,
            picture: None,
            compound: None,
            speed: None,
            color: None,
        }
    }

    #[test]
    fn the_nearest_audio_edit_is_that_of_the_nearest_cut_with_sound() {
        let t = Track {
            clips: vec![
                clip("c1", 0, 60, Some(8)),
                clip("c2", 60, 60, Some(0)),
                clip("c3", 120, 60, None),
            ],
            ..Track::new("V1", TrackKind::Video)
        };
        assert_eq!(nearest_edit(&t, 110, Side::Audio), Some(68));
    }
}
