//! The GUI's state and panels, apart from the eframe and OpenGL glue: so
//! the whole interface can run in tests (egui_kittest) against a live session
//! without a player.
//!
//! The UI is a thin view. Each frame it reads the session's latest
//! [`Snapshot`] and draws it; every user action becomes one request sent to the
//! session (AGENTS.md rule 3), the same requests the CLI and IPC clients send.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, Key, RichText};
use serde_json::{Value, json};
use vidcut_core::model::Project;
use vidcut_core::request::ViewerMode;
use vidcut_core::time::{Rational, format_timecode};

use super::theme;
use super::timeline::TimelineView;
use crate::live::{self, Filmstrip, PlayerId, Snapshot};

/// Text being typed into the viewer's timecode field (`=`, `+`, `-`).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct TimecodeEntry {
    /// `None`: absolute; `Some(1)` / `Some(-1)`: relative.
    pub sign: Option<i64>,
    pub text: String,
    pub focus: bool,
}

/// Pool thumbnails as textures.
pub(super) struct Thumbs {
    pub strip: Arc<Filmstrip>,
    pub textures: Vec<egui::TextureHandle>,
}

impl Thumbs {
    /// The picture nearest `secs` of the source.
    pub(super) fn nearest(&self, secs: f64) -> Option<egui::TextureId> {
        let i = self
            .strip
            .times
            .iter()
            .enumerate()
            .min_by(|a, b| (a.1 - secs).abs().total_cmp(&(b.1 - secs).abs()))?
            .0;
        self.textures.get(i).map(egui::TextureHandle::id)
    }
}

/// A drag in progress.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Drag {
    /// Moving a playhead.
    Scrub(PlayerId),
    /// Moving a source mark of `media`: In (true) or Out, now at `frame`.
    SourceMark {
        media: String,
        is_in: bool,
        frame: i64,
    },
    /// Moving a Source Tape span handle (decision D17). `frame` is the stored
    /// tape frame: In inclusive, Out exclusive.
    TapeMark { is_in: bool, frame: i64 },
}

/// What the viewer can show of the player (from the GPU glue).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Picture {
    pub texture: Option<egui::TextureId>,
    /// The texture holds a rendered frame.
    pub has_frame: bool,
}

pub struct Workspace {
    pub(super) live: live::Handle,
    pub(super) snap: Arc<Snapshot>,
    pub(super) thumbs: HashMap<String, Thumbs>,
    /// The player's picture, and the size (physical pixels) the viewer
    /// wants it rendered at.
    pub picture: Picture,
    pub want: [i32; 2],
    /// What the viewer's player does, while the viewer shows it (stats for
    /// nerds).
    pub stats: Option<live::stats::PlayerStats>,
    /// The GPU the window draws on, for the status bar's versions.
    pub display: Option<String>,
    /// The peaks of the sound the viewer plays now, while it plays.
    pub levels: Option<vidcut_av::player::Levels>,
    // View state (not part of the project).
    pub(super) search: String,
    /// Bins in the pool: drop targets, the order shown, renaming.
    pub(super) bins: super::pool::BinsUi,
    pub(super) timecode: Option<TimecodeEntry>,
    pub(super) duration_text: Option<String>,
    pub(super) show_log: bool,
    pub(super) log_edits_only: bool,
    /// Media under the pointer in the pool, with the hovered source frame.
    pub(super) hover: Option<(String, i64)>,
    /// Scrubbing in progress: (player, last frame sent).
    pub(super) scrub: Option<(PlayerId, i64)>,
    pub(super) recent: Vec<PathBuf>,
    /// A message from the GUI itself (not from the session) and when it came.
    pub(super) flash: Option<(String, std::time::Instant)>,
    /// The window title last set.
    title: String,
    /// Edits take only the video (the Video Only toggle).
    pub(super) video_only: bool,
    /// Edits take only the audio (Audio Only).
    pub(super) audio_only: bool,
    /// The target track picked in a track header; `None`: the
    /// default (Track 1, or A1 for sound alone).
    pub(super) target: Option<String>,
    /// What a press in the viewer's scroll bar is dragging.
    pub(super) drag: Option<Drag>,
    /// The timeline area's view state (zoom, options, the gesture in progress).
    pub(super) timeline: TimelineView,
    /// The edit the Trim Editor shows, if it is open.
    pub(super) trim_editor: Option<i64>,
    /// Exact frames for the Trim Editor and Dynamic Trim (created on first
    /// use).
    pub(super) stills: Option<super::stills::Stills>,
    /// Quick Export, the Export dialog, jobs and timeline interchange.
    pub(super) export: super::export::ExportUi,
    /// The Mixer panel is open, and its meters' state.
    pub(super) show_mixer: bool,
    pub(super) mixer: super::mixer::Mixer,
    /// File > Preferences.
    pub(super) preferences: super::preferences::PreferencesUi,
    pub(super) project_ui: super::project::ProjectUi,
    pub(super) import: super::import::ImportUi,
    pub(super) relink: super::relink::RelinkUi,
    pub(super) dialogs: Arc<dyn super::export::Dialogs>,
    /// Where file dialogs start, remembered across runs.
    pub(super) folders: super::folders::Folders,
    /// The timeline's filmstrip pictures (out while the timeline paints).
    pub(super) tiles: Option<super::tiles::Tiles>,
}

/// A moment in local time.
#[derive(Debug, Clone, Copy, PartialEq)]
struct LocalTime {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
}

impl LocalTime {
    /// `2026-10-12 14:32:05`.
    fn full(&self) -> String {
        format!(
            "{}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

/// `ms` (since 1970 UTC) in the computer's time zone.
fn local_time(ms: u64) -> Option<LocalTime> {
    let secs = libc::time_t::try_from(ms / 1000).ok()?;
    // SAFETY: localtime_r (localtime_s on Windows) writes only the `tm` it
    // is given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    #[cfg(unix)]
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return None;
    }
    #[cfg(windows)]
    if unsafe { libc::localtime_s(&mut tm, &secs) } != 0 {
        return None;
    }
    Some(LocalTime {
        year: tm.tm_year + 1900,
        month: u32::try_from(tm.tm_mon + 1).ok()?,
        day: u32::try_from(tm.tm_mday).ok()?,
        hour: u32::try_from(tm.tm_hour).ok()?,
        minute: u32::try_from(tm.tm_min).ok()?,
        second: u32::try_from(tm.tm_sec).ok()?,
    })
}

/// When the project last changed, for the status bar: the time if today,
/// else the date too.
fn edited_text(edited: &LocalTime, today: &LocalTime) -> String {
    if (edited.year, edited.month, edited.day) == (today.year, today.month, today.day) {
        format!("edited {:02}:{:02}", edited.hour, edited.minute)
    } else {
        format!(
            "edited {}-{:02}-{:02} {:02}:{:02}",
            edited.year, edited.month, edited.day, edited.hour, edited.minute
        )
    }
}

/// How the viewer's player decodes the picture shown, for the status bar,
/// from `player.state`'s `hwdec`, and what it means on hover. `None`
/// before it has shown a picture of a file.
fn decoding(hwdec: &str) -> Option<(String, String)> {
    let gpu = |api: &str| {
        (
            format!("decoding on the GPU ({api})"),
            format!(
                "The viewer decodes on the graphics card ({api}). File > Preferences: GPU \
                 turns it off"
            ),
        )
    };
    Some(match hwdec {
        "" => return None,
        "no" => (
            "decoding on the CPU".into(),
            "The viewer decodes in software: GPU acceleration or decoding is off in File > \
             Preferences, or the GPU can't decode this file"
                .into(),
        ),
        other => gpu(vidcut_av::hw::Kind::named(other).map_or(other, |k| k.label())),
    })
}

impl Workspace {
    pub fn new(live: live::Handle) -> Self {
        Self {
            snap: live.snapshot(),
            live,
            thumbs: HashMap::new(),
            picture: Picture::default(),
            stats: None,
            want: [640, 360],
            display: None,
            levels: None,
            search: String::new(),
            bins: Default::default(),
            timecode: None,
            duration_text: None,
            show_log: false,
            log_edits_only: true,
            hover: None,
            scrub: None,
            recent: recent_projects(),
            flash: None,
            title: String::new(),
            video_only: false,
            audio_only: false,
            target: None,
            drag: None,
            timeline: TimelineView {
                options: super::timeline::Options::load(),
                ..TimelineView::default()
            },
            trim_editor: None,
            stills: None,
            export: Default::default(),
            show_mixer: false,
            mixer: Default::default(),
            preferences: Default::default(),
            project_ui: Default::default(),
            import: Default::default(),
            relink: Default::default(),
            dialogs: Arc::new(super::export::RfdDialogs),
            folders: super::folders::Folders::load(),
            tiles: Some(super::tiles::Tiles::new(
                &crate::preferences::load().unwrap_or_default().thumbnails,
            )),
        }
    }

    /// The open project's folder.
    pub(super) fn project_folder(&self) -> Option<PathBuf> {
        self.snap
            .project
            .as_ref()
            .and_then(|p| p.path.parent().map(Path::to_path_buf))
            .filter(|d| !d.as_os_str().is_empty())
    }

    /// Opens a window (`gui.show`).
    pub fn show_panel(&mut self, panel: vidcut_core::request::GuiPanel, ctx: &egui::Context) {
        use vidcut_core::request::GuiPanel;
        if self.project().is_none()
            && matches!(
                panel,
                GuiPanel::QuickExport | GuiPanel::Export | GuiPanel::Mixer
            )
        {
            return;
        }
        match panel {
            GuiPanel::QuickExport => self.open_quick_export(ctx),
            GuiPanel::Export => self.open_export(ctx),
            GuiPanel::Jobs => self.export.show_jobs = true,
            GuiPanel::Log => self.show_log = true,
            GuiPanel::Mixer => self.show_mixer = true,
            GuiPanel::Preferences => self.open_preferences(ctx),
        }
    }

    /// Replaces the file dialogs (tests answer them themselves).
    pub fn set_dialogs(&mut self, dialogs: Arc<dyn super::export::Dialogs>) {
        self.dialogs = dialogs;
    }

    /// Where the timelines were drawn last frame (for tests).
    pub fn timeline_layout(&self) -> Option<super::timeline::Layout> {
        self.timeline.layout.clone()
    }

    /// How many timeline filmstrip pictures are kept, and their bytes (for
    /// tests).
    pub fn timeline_tiles(&self) -> (usize, usize) {
        self.tiles
            .as_ref()
            .map_or((0, 0), super::tiles::Tiles::usage)
    }

    /// How many timeline filmstrip pictures are on their way: asked for and
    /// not made yet, or being made (for tests).
    pub fn timeline_tiles_pending(&self) -> usize {
        self.tiles.as_ref().map_or(0, super::tiles::Tiles::pending)
    }

    /// The frame of the selected edit, if one is selected (for tests).
    pub fn selected_edit(&self) -> Option<i64> {
        self.timeline.edit.map(|e| e.at)
    }

    /// Timeline Options (for tests and settings).
    pub fn timeline_options_mut(&mut self) -> &mut super::timeline::Options {
        &mut self.timeline.options
    }

    // ---------------------------------------------------------------- helpers

    /// Sends one request to the session.
    pub(super) fn send(&self, method: &str, params: Value) {
        self.live.request(method, params);
    }

    /// Any timeline action shows the timeline in the viewer (spec §4.1):
    /// asked only when it isn't shown already, so the command log isn't
    /// full of requests that change nothing.
    pub(super) fn show_timeline(&self) {
        if self.snap.viewer != vidcut_core::request::ViewerMode::Timeline {
            self.send("viewer.set", json!({ "mode": "timeline" }));
        }
    }

    /// Sends the request that ends a drag on the timeline: a dropped clip
    /// doesn't move the playhead the way an edit function does (spec D15).
    pub(super) fn send_drag(&self, method: &str, params: Value) {
        self.live.request_drag(method, params);
    }

    pub(super) fn project(&self) -> Option<&Project> {
        self.snap.project()
    }

    /// The project, shared rather than copied: a copy each frame costs
    /// milliseconds on a big one.
    pub(super) fn project_shared(&self) -> Option<Arc<Project>> {
        self.snap.project.as_ref().map(|s| s.project.clone())
    }

    pub(super) fn fps(&self) -> Rational {
        self.project()
            .map_or(Rational::new(30, 1).expect("valid"), |p| p.settings.fps)
    }

    /// The frame numbers timecode skips at each minute but every tenth
    /// where the project counts drop-frame, else 0 (decision D47).
    pub(super) fn dropped(&self) -> i64 {
        match self.project() {
            Some(p) if p.settings.counts_drop_frame() => p.settings.fps.dropped_per_minute(),
            _ => 0,
        }
    }

    /// Record timecode of a timeline frame.
    pub(super) fn tc(&self, frame: i64) -> String {
        match self.project() {
            Some(p) => p.settings.timecode(frame),
            None => format_timecode(frame, self.fps(), 0, false),
        }
    }

    /// Source timecode (counting from the media's first frame).
    pub(super) fn src_tc(&self, frame: i64) -> String {
        match self.project() {
            Some(p) => p.settings.length_timecode(frame),
            None => format_timecode(frame, self.fps(), 0, false),
        }
    }

    pub(super) fn media_frames(&self, media: &str) -> i64 {
        self.project()
            .and_then(|p| p.media(media).map(|m| p.media_frames(m)))
            .unwrap_or(0)
    }

    /// Media ids of the Source Tape, in order.
    pub(super) fn tape_order(&self) -> Vec<String> {
        self.snap.tape.iter().map(|e| e.media.clone()).collect()
    }

    /// A Source Tape span that sits on more than one clip (decision D17),
    /// when the stored order is still this tape.
    pub(super) fn crossing_span(&self) -> Option<(i64, i64)> {
        let p = self.project()?;
        if p.tape_marks.order != self.tape_order() {
            return None;
        }
        vidcut_core::render::crossing_span(
            &self.snap.tape,
            p.tape_marks.mark_in,
            p.tape_marks.mark_out,
        )
    }

    /// Source ranges for an edit function (spec §7): the clip in the source
    /// viewer with its marks, or in Timeline mode the media selected in the
    /// pool. A Source Tape span across clips is one `tape` edit (D17).
    /// Ranges are explicit so the command log replays exactly.
    pub(super) fn edit_sources(&self) -> Vec<Value> {
        if self.snap.viewer == ViewerMode::SourceTape && self.crossing_span().is_some() {
            return vec![json!({ "tape": true })];
        }
        let Some(p) = self.project() else {
            return Vec::new();
        };
        let range = |media: &str| -> Option<Value> {
            let m = p.media(media)?;
            let frames = p.media_frames(m);
            let a = m.marks.mark_in.unwrap_or(0);
            let b = m.marks.mark_out.unwrap_or(frames);
            (b > a).then(|| json!({ "media": media, "range": format!("{a}f..{b}f") }))
        };
        match self.snap.source_at() {
            Some((media, _)) => range(media).into_iter().collect(),
            None => self
                .snap
                .selection
                .media
                .iter()
                .filter_map(|m| range(m))
                .collect(),
        }
    }

    /// Shows a message from the GUI itself in the status bar.
    pub(super) fn say(&mut self, text: impl Into<String>) {
        self.flash = Some((text.into(), std::time::Instant::now()));
    }

    /// Runs an edit function with the current source(s).
    pub(super) fn edit_function(&mut self, method: &str) {
        let sources = self.edit_sources();
        let playhead = self.snap.timeline.position;
        let with_position = |mut s: Value| {
            match method {
                "edit.smart_insert" => s["playhead"] = json!(playhead),
                "edit.overwrite" | "edit.ripple_overwrite" | "edit.place_on_top" => {
                    s["at"] = json!(playhead)
                }
                _ => {}
            }
            s
        };
        let (video_only, audio_only) = (self.video_only, self.audio_only);
        // Append always goes after Track 1's last clip (decision D3), and
        // Place On Top picks its own track.
        let target = self
            .target
            .clone()
            .filter(|_| !matches!(method, "edit.append" | "edit.place_on_top"))
            .filter(|t| self.project().is_some_and(|p| p.track(t).is_some()));
        let sources: Vec<Value> = sources
            .into_iter()
            .map(|mut s| {
                if video_only {
                    s["video_only"] = json!(true);
                }
                if audio_only {
                    s["audio_only"] = json!(true);
                }
                if let Some(t) = &target {
                    s["track"] = json!(t);
                }
                s
            })
            .collect();
        match sources.len() {
            0 => self.say("Pick a source first: open a clip, or select media in the pool"),
            1 => self.send(
                method,
                with_position(sources.into_iter().next().expect("one")),
            ),
            _ if method == "edit.append" => {
                let commands: Vec<Value> = sources
                    .into_iter()
                    .map(|s| json!({ "method": "edit.append", "params": s }))
                    .collect();
                let label = format!("append {} clips", commands.len());
                self.send("apply", json!({ "commands": commands, "label": label }));
            }
            _ => self.send(
                method,
                with_position(sources.into_iter().next().expect("one")),
            ),
        }
    }

    /// Seeks the viewer's player.
    pub(super) fn seek(&self, player: PlayerId, frame: i64, fast: bool) {
        let viewer = match player {
            PlayerId::Timeline => ViewerMode::Timeline,
            PlayerId::Source => self.snap.last_source,
        };
        self.send(
            "player.seek",
            json!({ "at": frame.max(0), "viewer": viewer, "fast": fast }),
        );
    }

    // ---------------------------------------------------------------- frame

    fn sync_textures(&mut self, ctx: &egui::Context) {
        let thumbs = self.snap.thumbs.clone();
        self.thumbs.retain(|m, _| thumbs.contains_key(m));
        for (media, strip) in thumbs.iter() {
            if self
                .thumbs
                .get(media)
                .is_some_and(|t| Arc::ptr_eq(&t.strip, strip))
            {
                continue;
            }
            let size = [strip.width as usize, strip.height as usize];
            let textures = strip
                .frames
                .iter()
                .enumerate()
                .map(|(i, rgba)| {
                    ctx.load_texture(
                        format!("thumb-{media}-{i}"),
                        egui::ColorImage::from_rgba_unmultiplied(size, rgba),
                        egui::TextureOptions::LINEAR,
                    )
                })
                .collect();
            self.thumbs.insert(
                media.clone(),
                Thumbs {
                    strip: strip.clone(),
                    textures,
                },
            );
        }
    }

    /// The newest message: the session's latest notice, or one of ours.
    fn notices(&mut self, ui: &mut egui::Ui) {
        if let Some((text, at)) = &self.flash {
            if at.elapsed().as_secs() < 6 {
                ui.label(RichText::new(text).color(theme::ACCENT));
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs(1));
                return;
            }
            self.flash = None;
        }
        if let Some(n) = self.snap.notices.back() {
            let color = if n.error {
                theme::ERROR
            } else {
                theme::TEXT_DIM
            };
            ui.label(RichText::new(&n.text).color(color));
        }
    }

    // ---------------------------------------------------------------- panels

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New Project…").clicked() {
                    self.open_new_project();
                }
                if ui.button("Open Project…").clicked() {
                    self.pick_project(ui.ctx(), None);
                }
                ui.menu_button("Open Recent", |ui| {
                    for p in self.recent.clone() {
                        if ui.button(p.display().to_string()).clicked() {
                            self.send("project.open", json!({ "path": p }));
                        }
                    }
                });
                ui.separator();
                let open = self.project().is_some();
                if ui
                    .add_enabled(open, egui::Button::new("Project Settings…"))
                    .clicked()
                {
                    self.open_project_settings();
                }
                ui.separator();
                if ui
                    .add_enabled(open, egui::Button::new("Import Media…"))
                    .clicked()
                {
                    self.import_media();
                }
                if ui
                    .add_enabled(open, egui::Button::new("New Bin"))
                    .on_hover_text(super::keys("Shift+Ctrl+N: a bin in the bin the pool shows"))
                    .clicked()
                {
                    self.new_bin();
                }
                if ui
                    .add_enabled(open, egui::Button::new("Import Timeline (OTIO)…"))
                    .clicked()
                {
                    self.import_timeline(ui.ctx(), false);
                }
                if ui
                    .add_enabled(
                        open,
                        egui::Button::new("Import Timeline, Decomposing Compound Clips…"),
                    )
                    .clicked()
                {
                    self.import_timeline(ui.ctx(), true);
                }
                if ui
                    .add_enabled(open, egui::Button::new("Export Timeline (OTIO)…"))
                    .clicked()
                {
                    self.export_timeline(ui.ctx(), false);
                }
                if ui
                    .add_enabled(
                        open,
                        egui::Button::new("Export Timeline with Media (OTIOZ)…"),
                    )
                    .clicked()
                {
                    self.export_timeline(ui.ctx(), true);
                }
                ui.separator();
                if ui
                    .add(egui::Button::new("Preferences…").shortcut_text(super::keys("Ctrl+,")))
                    .clicked()
                {
                    self.open_preferences(ui.ctx());
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Edit", |ui| {
                let (undo, redo) = self
                    .snap
                    .project
                    .as_ref()
                    .map(|p| (p.undo.first().cloned(), p.redo.first().cloned()))
                    .unwrap_or_default();
                if ui
                    .add_enabled(
                        undo.is_some(),
                        egui::Button::new(format!("Undo {}", undo.unwrap_or_default())),
                    )
                    .clicked()
                {
                    self.send("history.undo", json!({}));
                }
                if ui
                    .add_enabled(
                        redo.is_some(),
                        egui::Button::new(format!("Redo {}", redo.unwrap_or_default())),
                    )
                    .clicked()
                {
                    self.send("history.redo", json!({}));
                }
            });
            ui.menu_button("Timeline", |ui| {
                let mut on = self.snap.audio_scrubbing;
                if ui
                    .checkbox(&mut on, "Audio Scrubbing")
                    .on_hover_text(
                        "Shift+S: frame steps and drags of a paused playhead play the sound \
                         under it",
                    )
                    .clicked()
                {
                    self.send("player.audio_scrubbing", json!({ "on": on }));
                }
            });
            ui.separator();
            // The project's name is in the window title.
            if self.snap.project.is_some() {
                if ui
                    .button("Import")
                    .on_hover_text(super::keys("Ctrl+I"))
                    .clicked()
                {
                    self.import_media();
                }
                if ui
                    .button("Quick Export")
                    .on_hover_text("Export the In–Out range, or the whole timeline, with a preset")
                    .clicked()
                {
                    self.open_quick_export(ui.ctx());
                }
                if ui
                    .button("Export")
                    .on_hover_text("All export settings")
                    .clicked()
                {
                    self.open_export(ui.ctx());
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.toggle_value(&mut self.show_log, "Command log");
                if self.project().is_some() {
                    ui.toggle_value(&mut self.show_mixer, "Mixer")
                        .on_hover_text("Track faders, mute, solo and meters");
                }
                let running = self.snap.jobs.iter().filter(|j| j.unfinished()).count();
                let jobs = if running > 0 {
                    format!("Jobs ({running})")
                } else {
                    "Jobs".to_string()
                };
                ui.toggle_value(&mut self.export.show_jobs, jobs)
                    .on_hover_text("Exports and proxy encodes, with progress");
                if let Some(e) = &self.snap.preview_error {
                    ui.label(RichText::new("no preview").color(theme::ERROR))
                        .on_hover_text(e);
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            self.notices(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // What this is built on, for reports: the full versions on
                // hover.
                static VERSIONS: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();
                let (short, long) = VERSIONS.get_or_init(|| {
                    let mut long = format!(
                        "vidcut {}\nFFmpeg {}",
                        env!("CARGO_PKG_VERSION"),
                        vidcut_av::ffmpeg_version()
                    );
                    for (lib, v) in vidcut_av::library_versions() {
                        if lib != "version" {
                            long.push_str(&format!("\n  {lib} {v}"));
                        }
                    }
                    if let Some(gpu) = &self.display {
                        long.push_str(&format!("\nDrawing on {gpu}"));
                    }
                    let short = format!(
                        "vidcut {} · FFmpeg {}",
                        env!("CARGO_PKG_VERSION"),
                        vidcut_av::ffmpeg_release()
                    );
                    (short, long)
                });
                // Right to left: the versions, how the viewer decodes, live
                // control, the project's revision and format, with a line
                // between them.
                let dim = |text: String| RichText::new(text).color(theme::TEXT_DIM);
                ui.label(dim(short.clone())).on_hover_text(long.as_str());
                if let Some(socket) = &self.snap.socket {
                    ui.separator();
                    ui.label(dim(format!("pid {}", self.snap.pid)))
                        .on_hover_text(format!(
                            "Live control: {}\nvidcut CLI commands for this project run here",
                            socket.display()
                        ));
                }
                if let Some((text, tip)) = decoding(&self.snap.active().hwdec) {
                    ui.separator();
                    ui.label(dim(text)).on_hover_text(tip);
                }
                if let Some(p) = self.project() {
                    let fps = p.settings.fps;
                    ui.separator();
                    let revision = format!(
                        "Revision {}: one up with every change. Scripts give it with \
                         --if-rev so they never overwrite a newer edit",
                        p.rev
                    );
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis() as u64);
                    match (p.edited.and_then(local_time), local_time(now)) {
                        (Some(edited), Some(today)) => {
                            ui.label(dim(edited_text(&edited, &today)))
                                .on_hover_text(format!(
                                    "Last changed {}, local time\n{revision}",
                                    edited.full()
                                ));
                        }
                        _ => {
                            ui.label(dim(format!("revision {}", p.rev)))
                                .on_hover_text(revision);
                        }
                    }
                    ui.separator();
                    let format = ui.label(dim(format!(
                        "{}×{}, {} fps",
                        p.settings.width,
                        p.settings.height,
                        fps.decimal()
                    )));
                    if !fps.is_whole() {
                        format.on_hover_text(format!("{fps} fps"));
                    }
                }
            });
        });
    }

    fn start_screen(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.25);
            ui.label(RichText::new("vidcut").size(36.0).strong());
            ui.label(RichText::new("a scriptable video editor").color(theme::TEXT_DIM));
            ui.add_space(24.0);
            ui.horizontal(|ui| {
                ui.add_space((ui.available_width() - 260.0).max(0.0) / 2.0);
                if ui.button("New Project…").clicked() {
                    self.open_new_project();
                }
                if ui.button("Open Project…").clicked() {
                    self.pick_project(ui.ctx(), None);
                }
            });
            if !self.recent.is_empty() {
                ui.add_space(24.0);
                ui.label(RichText::new("Recent").color(theme::TEXT_DIM));
                for p in self.recent.clone() {
                    if ui.link(p.display().to_string()).clicked() {
                        self.send("project.open", json!({ "path": p }));
                    }
                }
            }
        });
    }

    fn log_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Command log");
            ui.checkbox(&mut self.log_edits_only, "edits only");
        });
        ui.label(
            RichText::new(
                "What you, scripts and agents did. Hover an entry for its request; click to copy it as a command.",
            )
            .color(theme::TEXT_DIM)
            .small(),
        );
        ui.separator();
        let project = self.snap.project.as_ref().map(|p| p.path.clone());
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for e in self.snap.log.iter() {
                    if self.log_edits_only && !e.changed && e.ok {
                        continue;
                    }
                    let color = if e.ok { theme::TEXT } else { theme::ERROR };
                    let params = if e.params.as_object().is_some_and(|o| o.is_empty()) {
                        String::new()
                    } else {
                        e.params.to_string()
                    };
                    // Who: nothing for the person at the GUI.
                    let who = match e.origin.as_str() {
                        "ipc" => "script",
                        "session" => "automatic",
                        _ => "",
                    };
                    let r = ui
                        .horizontal_wrapped(|ui| {
                            if !who.is_empty() {
                                ui.label(RichText::new(who).small().color(theme::TEXT_DIM));
                            }
                            ui.add(
                                egui::Label::new(RichText::new(&e.summary).small().color(color))
                                    .sense(egui::Sense::click()),
                            )
                        })
                        .inner;
                    let request = format!("{} {}", e.method, params);
                    let r = match &e.error {
                        Some(err) => r.on_hover_text(format!("{err}\n{request}")),
                        None => {
                            r.on_hover_text(format!("{request}\nclick: copy as a vidcut command"))
                        }
                    };
                    if r.clicked() {
                        let cli = match &project {
                            Some(p) => format!(
                                "vidcut -p {} rpc {} {}",
                                shell_quote(&p.display().to_string()),
                                e.method,
                                shell_quote(&e.params.to_string())
                            ),
                            None => format!(
                                "vidcut rpc {} {}",
                                e.method,
                                shell_quote(&e.params.to_string())
                            ),
                        };
                        ui.ctx().copy_text(cli);
                    }
                }
            });
    }

    // ---------------------------------------------------------------- dialogs

    /// Asks for a project file to open, or with `new` to create with those
    /// `project.open` params (its format), on a helper thread: portal
    /// dialogs block. The dialog starts where a project was last opened or
    /// created, else in the open project's folder, else the most recent
    /// project's. An OpenTimelineIO file is read there too, for the
    /// questions before it opens.
    pub(super) fn pick_project(&self, ctx: &egui::Context, new: Option<Value>) {
        use super::folders::Kind;
        let (live, dialogs, folders) = (
            self.live.clone(),
            self.dialogs.clone(),
            self.folders.clone(),
        );
        let (found, ctx) = (self.project_ui.found.clone(), ctx.clone());
        let folder = folders
            .get(Kind::Projects)
            .or_else(|| self.project_folder())
            .or_else(|| {
                self.recent
                    .first()
                    .and_then(|p| p.parent().map(Path::to_path_buf))
            })
            .unwrap_or_default();
        let _ = crate::threads::spawn(
            "vidcut-project-dialog",
            crate::threads::Work::Interactive,
            move || {
                let path = if new.is_some() {
                    dialogs.save_file("New Project", &folder, "untitled.vidcut", &["vidcut"])
                } else {
                    dialogs.pick_file("Open Project", &folder, &["vidcut", "otio", "otioz"])
                };
                if let Some(path) = path
                    .as_ref()
                    .filter(|p| new.is_none() && vidcut_core::editor::is_timeline_file(p))
                {
                    folders.remember(Kind::Projects, path);
                    let file = super::project::OtioFile::read(path.clone());
                    *found.lock().unwrap_or_else(|e| e.into_inner()) = Some(file);
                    ctx.request_repaint();
                } else if let Some(mut path) = path {
                    let mut params = json!({ "path": path });
                    if let Some(format) = new {
                        // The save dialog asked before replacing the file it
                        // names, but not one named by an extension added here.
                        let force = path.extension().is_some();
                        if path.extension().is_none() {
                            path.set_extension("vidcut");
                        }
                        params = json!({ "path": path, "create": true, "force": force });
                        if let (Some(p), Some(f)) = (params.as_object_mut(), format.as_object()) {
                            p.extend(f.clone());
                        }
                    }
                    folders.remember(Kind::Projects, &path);
                    live.request("project.open", params);
                }
            },
        );
    }

    // ---------------------------------------------------------------- keys

    /// One of the workspace's questions (modal dialogs) is up (tests read
    /// it too).
    pub fn asking(&self) -> bool {
        self.import.asking() || self.export.asking(&self.snap.jobs)
    }

    /// Keyboard shortcuts (spec §11). Ignored while a widget has focus or a
    /// question is up: they are read before the windows are drawn, so a
    /// modal doesn't stop them itself.
    fn keys(&mut self, ctx: &egui::Context) {
        // `VIDCUT_DEBUG` traces key presses too: what a keyboard
        // layout sends for a shortcut.
        if vidcut_av::debug() {
            ctx.input(|i| {
                for e in &i.events {
                    if let egui::Event::Key {
                        key,
                        physical_key,
                        pressed: true,
                        modifiers,
                        ..
                    } = e
                    {
                        eprintln!("[keys] {key:?} (physical {physical_key:?}) {modifiers:?}");
                    }
                }
            });
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if self.asking() {
            // Panels drawn after this read keys too (a drag in progress
            // reads N), so nothing behind the question sees them. A pressed
            // Escape stays for the question.
            ctx.input_mut(|i| {
                i.events.retain(|e| match e {
                    egui::Event::Key {
                        key: Key::Escape,
                        pressed: true,
                        modifiers,
                        ..
                    } => !modifiers.any(),
                    egui::Event::Key { .. }
                    | egui::Event::Text(_)
                    | egui::Event::Copy
                    | egui::Event::Cut
                    | egui::Event::Paste(_) => false,
                    _ => true,
                });
            });
            return;
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::Comma)) {
            self.open_preferences(ctx);
        }
        if self.snap.project.is_none() {
            return;
        }
        let input = ctx.input(|i| i.clone());
        let m = input.modifiers;
        let pressed = |k: Key| input.key_pressed(k);
        let viewer = self.snap.viewer;
        let id = PlayerId::of(viewer);
        let player = self.snap.player(id).clone();
        let fps = self.fps().as_f64().round().max(1.0) as i64;
        self.timeline_keys(&input);

        // Clipboard: egui turns Ctrl+C/X/V into events.
        for e in &input.events {
            match e {
                egui::Event::Copy | egui::Event::Cut if !self.snap.selection.clips.is_empty() => {
                    let clips = self.snap.selection.clips.clone();
                    // Also put the clips on the system clipboard as JSON (paste
                    // them into another project with `edit paste --clips`).
                    if let Some(p) = self.project()
                        && let Ok(data) = vidcut_core::ops::copy_clips(p, &clips)
                    {
                        ctx.copy_text(json!(data).to_string());
                    }
                    let method = if matches!(e, egui::Event::Cut) {
                        "edit.cut"
                    } else {
                        "edit.copy"
                    };
                    self.send(method, json!({ "clips": clips }));
                }
                egui::Event::Paste(_) => self.send("edit.paste", json!({})),
                _ => {}
            }
        }

        // Transport; in Dynamic Trim mode J/K/L and Space trim instead.
        let dynamic = self.timeline.dynamic.is_some();
        if dynamic {
            self.dynamic_keys(&input);
        }
        if pressed(Key::Space) && !dynamic {
            self.send("player.toggle", json!({}));
        }
        // Audio scrubbing on and off (decision D42).
        if pressed(Key::S) && m.shift && !m.command && !m.alt {
            self.send("player.audio_scrubbing", json!({}));
        }
        if pressed(Key::K) && !dynamic {
            self.send("player.pause", json!({}));
        }
        let k_down = input.key_down(Key::K);
        for (key, dir) in [(Key::L, 1.0), (Key::J, -1.0)]
            .into_iter()
            .filter(|_| !dynamic)
        {
            if pressed(key) {
                if k_down {
                    self.send("player.step", json!({ "frames": dir as i64 }));
                } else {
                    // Repeated J/L doubles the speed.
                    let speed = if player.speed * dir > 0.0 {
                        (player.speed * 2.0).clamp(-8.0, 8.0)
                    } else {
                        dir
                    };
                    self.send("player.play", json!({ "speed": speed }));
                }
            }
        }
        if pressed(Key::ArrowLeft) {
            let n = if m.shift { fps } else { 1 };
            self.send("player.step", json!({ "frames": -n }));
        }
        if pressed(Key::ArrowRight) {
            let n = if m.shift { fps } else { 1 };
            self.send("player.step", json!({ "frames": n }));
        }
        if pressed(Key::Home) {
            self.seek(id, 0, false);
        }
        if pressed(Key::End) {
            self.seek(id, player.frames, false);
        }
        if pressed(Key::ArrowUp) || pressed(Key::ArrowDown) {
            let up = pressed(Key::ArrowUp);
            // Dynamic Trim moves along the track of its edit, on
            // the timeline whatever the viewer shows.
            let dynamic_track = self.timeline.edit.filter(|_| dynamic).map(|e| e.track);
            let (id, player) = match dynamic_track {
                Some(_) => (
                    PlayerId::Timeline,
                    self.snap.player(PlayerId::Timeline).clone(),
                ),
                None => (id, player.clone()),
            };
            let points = match (m.shift, dynamic_track) {
                (true, _) => self.marker_points(),
                (false, Some(t)) => self.track_edit_points(&t.id()),
                (false, None) => self.edit_points(id),
            };
            let target = if up {
                points.iter().rev().find(|&&f| f < player.position)
            } else {
                points.iter().find(|&&f| f > player.position)
            };
            if let Some(&f) = target {
                self.seek(id, f, false);
                // Dynamic Trim moves on to that edit.
                if dynamic && !m.shift {
                    self.dynamic_commit();
                    let (side, track) = self.timeline.edit.map_or(
                        (
                            crate::gui::timeline::Side::Both,
                            crate::gui::timeline::TrackRef::MAIN,
                        ),
                        |e| (e.side, e.track),
                    );
                    // Up/Down lands on a picture cut. An audio selection stays
                    // on that cut's J/L handle (D20).
                    let at = if side == crate::gui::timeline::Side::Audio {
                        self.project()
                            .and_then(|p| p.track(&track.id()))
                            .map(|t| crate::gui::timeline::audio_at_cut(t, f))
                            .unwrap_or(f)
                    } else {
                        f
                    };
                    self.timeline.edit = Some(crate::gui::timeline::EditSel { at, side, track });
                }
            }
        }
        if pressed(Key::Q) && !m.command {
            let mode = if viewer == ViewerMode::Timeline {
                self.snap.last_source
            } else {
                ViewerMode::Timeline
            };
            self.send("viewer.set", json!({ "mode": mode }));
        }

        // Marks. Out marks the frame under the playhead as the last
        // one in the range; ranges are half-open, so the stored Out is one more.
        // (Ctrl+I imports.)
        if (pressed(Key::I) || pressed(Key::O)) && !m.command {
            let is_in = pressed(Key::I);
            if m.shift {
                self.go_to_mark(is_in);
            } else {
                self.mark(is_in);
            }
        }
        if pressed(Key::X) && m.alt {
            self.clear_marks();
        }

        // Edit functions (§7, §11).
        if pressed(Key::F9) {
            self.edit_function("edit.smart_insert");
        }
        if pressed(Key::F10) {
            self.edit_function(if m.shift {
                "edit.ripple_overwrite"
            } else {
                "edit.overwrite"
            });
        }
        if pressed(Key::F12) {
            self.edit_function(if m.shift {
                "edit.append"
            } else {
                "edit.place_on_top"
            });
        }
        // Ctrl+\ splits; Ctrl+B too, which every keyboard layout
        // types without AltGr.
        if (pressed(Key::Backslash) || pressed(Key::B)) && m.command {
            self.send(
                "edit.split",
                json!({ "at": self.snap.timeline.position, "clips": self.snap.selection.clips }),
            );
        }
        if (pressed(Key::Delete) || pressed(Key::Backspace)) && !m.command {
            self.delete(m.alt);
        }
        if pressed(Key::Z) && m.command {
            self.send(
                if m.shift {
                    "history.redo"
                } else {
                    "history.undo"
                },
                json!({}),
            );
        }
        if pressed(Key::I) && m.command {
            self.import_media();
        }
        if pressed(Key::N) && m.command && m.shift {
            self.new_bin();
        }
        if pressed(Key::M) && !m.command {
            if m.alt {
                let at = self.snap.timeline.position;
                if let Some(k) = self
                    .project()
                    .and_then(|p| p.markers.iter().find(|k| k.at == at))
                {
                    self.send("markers.remove", json!({ "marker": k.id }));
                }
            } else {
                // M on a marker edits it; elsewhere it adds one.
                let at = self.snap.timeline.position;
                let existing = self
                    .project()
                    .and_then(|p| p.markers.iter().find(|k| k.at == at))
                    .map(|k| k.id.clone());
                match (existing, self.project_shared()) {
                    (Some(id), Some(p)) => self.open_marker(id, &p),
                    _ => self.send("markers.add", json!({ "at": at })),
                }
            }
        }
        if pressed(Key::D) && !m.command && !self.snap.selection.clips.is_empty() {
            let enabled = self
                .project()
                .and_then(|p| p.clip(&self.snap.selection.clips[0]))
                .is_none_or(|c| c.enabled);
            self.send(
                "clip.set",
                json!({ "clips": self.snap.selection.clips, "enabled": !enabled }),
            );
        }
        if pressed(Key::A) && m.command {
            let clips: Vec<String> = if m.shift {
                Vec::new()
            } else {
                self.project()
                    .map(|p| {
                        p.tracks
                            .iter()
                            .flat_map(|t| &t.clips)
                            .map(|c| c.id.clone())
                            .collect()
                    })
                    .unwrap_or_default()
            };
            self.send("selection.set", json!({ "clips": clips }));
        }
        if pressed(Key::V) && m.alt && m.shift {
            let at = self.snap.timeline.position;
            let clips: Vec<String> = self
                .project()
                .map(|p| {
                    p.tracks
                        .iter()
                        .filter_map(|t| t.clip_at(at).map(|i| t.clips[i].id.clone()))
                        .collect()
                })
                .unwrap_or_default();
            self.send("selection.set", json!({ "clips": clips }));
        }

        // Timecode entry: `=` absolute, `+`/`-` relative.
        let typed: Option<char> = input.events.iter().find_map(|e| match e {
            egui::Event::Text(t) if t == "=" || t == "+" || t == "-" => t.chars().next(),
            _ => None,
        });
        if let Some(c) = typed {
            self.timecode = Some(TimecodeEntry {
                sign: match c {
                    '+' => Some(1),
                    '-' => Some(-1),
                    _ => None,
                },
                text: String::new(),
                focus: true,
            });
        }
    }

    /// Edit points to jump between with Up/Down.
    /// The edit points of one track (its clip edges), in order.
    fn track_edit_points(&self, track: &str) -> Vec<i64> {
        self.project()
            .and_then(|p| p.track(track))
            .map(|t| {
                let mut v = t.edit_points();
                v.push(t.end());
                v.dedup();
                v
            })
            .unwrap_or_default()
    }

    fn edit_points(&self, id: PlayerId) -> Vec<i64> {
        match id {
            PlayerId::Timeline => self
                .project()
                .and_then(|p| p.track("V1"))
                .map(|t| {
                    let mut v = t.edit_points();
                    v.push(t.end());
                    v.dedup();
                    v
                })
                .unwrap_or_default(),
            PlayerId::Source => match self.snap.viewer {
                ViewerMode::SourceTape => {
                    let mut v: Vec<i64> = self.snap.tape.iter().map(|e| e.start).collect();
                    if let Some((a, b)) = self.crossing_span() {
                        v.extend([a, b]);
                    }
                    v.sort_unstable();
                    v.dedup();
                    v
                }
                _ => {
                    let mut v = vec![0, self.snap.source.frames];
                    if let Some((m, _)) = self.snap.source_at()
                        && let Some(media) = self.project().and_then(|p| p.media(m))
                    {
                        v.extend(media.marks.mark_in);
                        v.extend(media.marks.mark_out);
                    }
                    v.sort_unstable();
                    v
                }
            },
        }
    }

    fn marker_points(&self) -> Vec<i64> {
        self.project()
            .map(|p| p.markers.iter().map(|k| k.at).collect())
            .unwrap_or_default()
    }

    /// I/O: marks the source clip in Source Clip mode, the tape in Source
    /// Tape mode (decision D17), the timeline otherwise. While the pointer
    /// is over a pool thumbnail, marks that clip there.
    pub(super) fn mark(&self, is_in: bool) {
        if self.hover.is_none() && self.snap.viewer == ViewerMode::SourceTape {
            let order = self.tape_order();
            if order.is_empty() {
                return;
            }
            let frame = self.snap.source.position;
            let value = if is_in { frame } else { frame + 1 };
            let key = if is_in { "in" } else { "out" };
            self.send("tape.marks.set", json!({ key: value, "order": order }));
            return;
        }
        let (media, frame) = match (&self.hover, self.snap.source_at()) {
            (Some((m, f)), _) => (Some(m.clone()), *f),
            (None, Some((m, f))) => (Some(m.to_string()), f),
            (None, None) => (None, self.snap.timeline.position),
        };
        let key = if is_in { "in" } else { "out" };
        let value = if is_in { frame } else { frame + 1 };
        match media {
            Some(m) => {
                let frames = self.media_frames(&m);
                self.send(
                    "media.marks.set",
                    json!({ "media": m, key: value.clamp(0, frames) }),
                );
            }
            None => self.send("marks.set", json!({ key: value })),
        }
    }

    fn go_to_mark(&self, is_in: bool) {
        if self.snap.viewer == ViewerMode::SourceTape
            && let Some((a, b)) = self.crossing_span()
        {
            self.seek(PlayerId::Source, if is_in { a } else { b - 1 }, false);
            return;
        }
        match self.snap.source_at() {
            Some((media, _)) => {
                let Some(m) = self.project().and_then(|p| p.media(media)) else {
                    return;
                };
                let base = match self.snap.viewer {
                    ViewerMode::SourceTape => self
                        .snap
                        .tape
                        .iter()
                        .find(|e| e.media == media)
                        .map_or(0, |e| e.start),
                    _ => 0,
                };
                let mark = if is_in {
                    m.marks.mark_in
                } else {
                    m.marks.mark_out.map(|o| o - 1)
                };
                if let Some(f) = mark {
                    self.seek(PlayerId::Source, base + f, false);
                }
            }
            None => {
                let marks = self.project().map(|p| p.marks.clone()).unwrap_or_default();
                let mark = if is_in {
                    marks.mark_in
                } else {
                    marks.mark_out.map(|o| o - 1)
                };
                if let Some(f) = mark {
                    self.seek(PlayerId::Timeline, f, false);
                }
            }
        }
    }

    pub(super) fn clear_marks(&self) {
        if self.snap.viewer == ViewerMode::SourceTape {
            let media = self.snap.source_at().map(|(m, _)| m.to_string());
            self.send("tape.marks.clear", json!({ "media": media }));
            return;
        }
        match self.snap.source_at() {
            Some((media, _)) => self.send("media.marks.clear", json!({ "media": media })),
            None => self.send("marks.clear", json!({})),
        }
    }

    /// Delete: the timeline In–Out range if set, else the selected
    /// clips. Ripples Track 1 unless `lift` (Alt, decision D9).
    fn delete(&self, lift: bool) {
        let Some(p) = self.project() else { return };
        if let (Some(a), Some(b)) = (p.marks.mark_in, p.marks.mark_out) {
            self.send(
                "edit.delete_range",
                json!({ "range": format!("{a}f..{b}f"), "lift": lift }),
            );
        } else if !self.snap.selection.clips.is_empty() {
            self.send(
                "edit.delete",
                json!({ "clips": self.snap.selection.clips, "lift": lift }),
            );
        }
    }

    /// Draws everything and handles input for one frame.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.snap = self.live.snapshot();
        // Removing a track renumbers the ones above it: a selected edit or
        // a Dynamic Trim run may now name another track, so they end.
        let tracks = self.project().map_or(0, |p| p.tracks.len());
        if tracks < self.timeline.tracks {
            self.timeline.edit = None;
            self.timeline.dynamic = None;
        }
        self.timeline.tracks = tracks;
        // An undo or someone else's edit can take the selected cut away:
        // the selection, and a Dynamic Trim of it, end with it.
        let rev = self.project().map_or(0, |p| p.rev);
        if rev != self.timeline.rev {
            self.timeline.rev = rev;
            if let (Some(e), Some(p)) = (self.timeline.edit, self.shown_project())
                && !crate::gui::timeline::edit_exists(&p, &e)
            {
                self.timeline.edit = None;
                self.timeline.dynamic = None;
            }
        }
        let title = match &self.snap.project {
            Some(p) => format!(
                "{} — vidcut",
                p.path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ),
            None => "vidcut".to_string(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        self.sync_textures(&ctx);
        if let Some(tiles) = &mut self.tiles {
            tiles.begin_frame(&ctx, self.snap.project());
        }
        self.keys(&ctx);
        self.export_windows(&ctx);
        self.preferences_window(&ctx);
        self.project_windows(&ctx);
        self.import_window(&ctx);
        self.relink_window(&ctx);
        egui::Panel::top("top")
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(4))
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(4))
            .show(ui, |ui| self.status_bar(ui));
        if self.show_log {
            egui::Panel::right("log")
                .resizable(true)
                .default_size(380.0)
                .show(ui, |ui| self.log_panel(ui));
        }
        if self.snap.project.is_none() {
            egui::CentralPanel::default().show(ui, |ui| self.start_screen(ui));
            return;
        }
        egui::Panel::bottom("timeline")
            .resizable(true)
            .default_size(300.0)
            .min_size(140.0)
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(6))
            .show(ui, |ui| self.timeline_panel(ui));
        if self.show_mixer {
            egui::Panel::right("mixer")
                .resizable(true)
                .default_size(320.0)
                .show(ui, |ui| self.mixer_panel(ui));
        }
        egui::Panel::left("pool")
            .resizable(true)
            .default_size(430.0)
            .min_size(260.0)
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(6))
            .show(ui, |ui| self.pool_panel(ui));
        // The timeline and the pool have asked for their tiles.
        if let Some(tiles) = &mut self.tiles {
            tiles.end_frame();
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(6))
            .show(ui, |ui| self.viewer_panel(ui));
        self.media_drag_overlay(&ctx);
    }
}

fn recent_projects() -> Vec<PathBuf> {
    std::fs::read_to_string(crate::paths::state_dir().join("recent.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<PathBuf>>(&t).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.exists())
        .collect()
}

fn shell_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./:@%+=,".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Fits `aspect` (width / height) inside `avail`, centered.
pub(super) fn fit(avail: egui::Rect, aspect: f32) -> egui::Rect {
    let (w, h) = (avail.width(), avail.height());
    let size = if w / h.max(1.0) > aspect {
        egui::vec2(h * aspect, h)
    } else {
        egui::vec2(w, w / aspect.max(0.01))
    };
    egui::Rect::from_center_size(avail.center(), size)
}

pub(super) fn dim(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).color(theme::TEXT_DIM)
}

/// Escape on a question with no popup of its own open: it dismisses the
/// question without acting (Cancel, or Keep Running on the quit question).
/// Consumed, so nothing behind the question sees it.
pub(super) fn escape_cancels<T>(ctx: &egui::Context, modal: &egui::ModalResponse<T>) -> bool {
    modal.is_top_modal
        && !modal.any_popup_open
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape))
}

#[cfg(test)]
mod tests {
    use super::decoding;

    #[test]
    fn the_status_bar_says_where_the_viewer_decodes() {
        assert_eq!(decoding(""), None);
        assert_eq!(decoding("no").unwrap().0, "decoding on the CPU");
        assert_eq!(decoding("nvdec").unwrap().0, "decoding on the GPU (NVDEC)");
        assert_eq!(
            decoding("d3d11va").unwrap().0,
            "decoding on the GPU (Direct3D 11)"
        );
    }
}
