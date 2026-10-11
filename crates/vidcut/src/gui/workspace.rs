//! The GUI's state and panels, apart from the eframe and OpenGL glue: so
//! the whole interface can run in tests (egui_kittest) against a live session
//! without players.
//!
//! The UI is a thin view. Each frame it reads the session's latest
//! [`Snapshot`] and draws it; every user action becomes one request sent to the
//! session (AGENTS.md rule 2), the same requests the CLI and IPC clients send.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{self, Key, RichText};
use serde_json::{Value, json};
use vidcut_core::model::Project;
use vidcut_core::request::ViewerMode;
use vidcut_core::time::{Rational, format_timecode};

use super::theme;
use super::timeline::TimelineView;
use crate::live::{self, Filmstrip, PlayerId, Snapshot};

/// Text being typed into the viewer's timecode field (`=`, `+`, `-`, p. 688).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct TimecodeEntry {
    /// `None`: absolute; `Some(1)` / `Some(-1)`: relative.
    pub sign: Option<i64>,
    pub text: String,
    pub focus: bool,
}

/// Pool thumbnails as textures.
pub(super) struct Thumbs {
    strip: Arc<Filmstrip>,
    pub textures: Vec<egui::TextureHandle>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PoolView {
    Thumbnails,
    List,
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

/// What the viewer can show of one player (from the GL glue).
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
    /// The players' pictures, and the sizes (physical pixels) the viewer
    /// wants them rendered at.
    pub pictures: [Picture; 2],
    pub wants: [[i32; 2]; 2],
    // View state (not part of the project).
    pub(super) pool_view: PoolView,
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
    /// Edits take only the video (the Cut page's Video Only toggle, p. 694).
    pub(super) video_only: bool,
    /// Edits take only the audio (Audio Only, p. 694).
    pub(super) audio_only: bool,
    /// The target track picked in a track header (p. 696); `None`: the
    /// default (Track 1, or A1 for sound alone).
    pub(super) target: Option<String>,
    /// What a press in the viewer's scroll bar is dragging.
    pub(super) drag: Option<Drag>,
    /// The timeline area's view state (zoom, options, the gesture in progress).
    pub(super) timeline: TimelineView,
    /// The edit the Trim Editor shows, if it is open (p. 729).
    pub(super) trim_editor: Option<i64>,
    /// Exact frames for the Trim Editor (created on first use).
    pub(super) stills: Option<super::stills::Stills>,
    /// Quick Export, the Export dialog, jobs and timeline interchange.
    pub(super) export: super::export::ExportUi,
    /// The Mixer panel is open (p. 649), and its meters' state.
    pub(super) show_mixer: bool,
    pub(super) mixer: super::mixer::Mixer,
    /// File > Preferences.
    pub(super) preferences: super::preferences::PreferencesUi,
    pub(super) import: super::import::ImportUi,
    pub(super) dialogs: Arc<dyn super::export::Dialogs>,
}

impl Workspace {
    pub fn new(live: live::Handle) -> Self {
        Self {
            snap: live.snapshot(),
            live,
            thumbs: HashMap::new(),
            pictures: [Picture::default(); 2],
            wants: [[640, 360]; 2],
            pool_view: PoolView::Thumbnails,
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
            import: Default::default(),
            dialogs: Arc::new(super::export::RfdDialogs),
        }
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

    /// Timeline Options (for tests and settings).
    pub fn timeline_options_mut(&mut self) -> &mut super::timeline::Options {
        &mut self.timeline.options
    }

    // ---------------------------------------------------------------- helpers

    /// Sends one request to the session.
    pub(super) fn send(&self, method: &str, params: Value) {
        self.live.request(method, params);
    }

    pub(super) fn project(&self) -> Option<&Project> {
        self.snap.project()
    }

    pub(super) fn fps(&self) -> Rational {
        self.project()
            .map_or(Rational::new(30, 1).expect("valid"), |p| p.settings.fps)
    }

    /// Record timecode of a timeline frame.
    pub(super) fn tc(&self, frame: i64) -> String {
        let start = self.project().map_or(0, |p| p.settings.start_tc);
        format_timecode(frame, self.fps(), start)
    }

    /// Source timecode (counting from the media's first frame).
    pub(super) fn src_tc(&self, frame: i64) -> String {
        format_timecode(frame, self.fps(), 0)
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
                    self.pick_project(true);
                }
                if ui.button("Open Project…").clicked() {
                    self.pick_project(false);
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
                    .add_enabled(open, egui::Button::new("Import Media…"))
                    .clicked()
                {
                    self.import_media();
                }
                if ui
                    .add_enabled(open, egui::Button::new("New Bin"))
                    .on_hover_text("Shift+Ctrl+N: a bin in the bin the pool shows")
                    .clicked()
                {
                    self.new_bin();
                }
                if ui
                    .add_enabled(open, egui::Button::new("Import Timeline (OTIO)…"))
                    .clicked()
                {
                    self.import_timeline(ui.ctx());
                }
                if ui
                    .add_enabled(open, egui::Button::new("Export Timeline (OTIO)…"))
                    .clicked()
                {
                    self.export_timeline(ui.ctx());
                }
                ui.separator();
                if ui
                    .add_enabled(open, egui::Button::new("Quick Export…"))
                    .clicked()
                {
                    self.open_quick_export(ui.ctx());
                }
                if ui.add_enabled(open, egui::Button::new("Export…")).clicked() {
                    self.open_export(ui.ctx());
                }
                ui.separator();
                if ui
                    .add(egui::Button::new("Preferences…").shortcut_text("Ctrl+,"))
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
            ui.separator();
            if let Some(p) = &self.snap.project {
                let name = p
                    .path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                ui.label(RichText::new(name).strong())
                    .on_hover_text(p.path.display().to_string());
                if ui.button("Import").on_hover_text("Ctrl+I").clicked() {
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
                        .on_hover_text("Track faders, mute, solo and meters (p. 649)");
                }
                let running = self.snap.jobs.iter().filter(|j| j.running()).count();
                let jobs = if running > 0 {
                    format!("Jobs ({running})")
                } else {
                    "Jobs".to_string()
                };
                ui.toggle_value(&mut self.export.show_jobs, jobs)
                    .on_hover_text("Exports, with progress");
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
                if let Some(socket) = &self.snap.socket {
                    ui.label(
                        RichText::new(format!("pid {}", self.snap.pid)).color(theme::TEXT_DIM),
                    )
                    .on_hover_text(format!(
                        "live control: {}\nvidcut CLI commands for this project run here",
                        socket.display()
                    ));
                }
                let hw = &self.snap.active().hwdec;
                if !hw.is_empty() {
                    ui.label(RichText::new(format!("hwdec {hw}")).color(theme::TEXT_DIM));
                }
                if let Some(p) = self.project() {
                    ui.label(
                        RichText::new(format!(
                            "{}x{} {} fps  rev {}",
                            p.settings.width, p.settings.height, p.settings.fps, p.rev
                        ))
                        .color(theme::TEXT_DIM),
                    );
                }
            });
        });
    }

    fn start_screen(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.25);
            ui.label(RichText::new("vidcut").size(36.0).strong());
            ui.label(RichText::new("a scriptable cut-page editor").color(theme::TEXT_DIM));
            ui.add_space(24.0);
            ui.horizontal(|ui| {
                ui.add_space((ui.available_width() - 260.0).max(0.0) / 2.0);
                if ui.button("New Project…").clicked() {
                    self.pick_project(true);
                }
                if ui.button("Open Project…").clicked() {
                    self.pick_project(false);
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
                "Every action as the request it sent; copy one to replay it from a shell.",
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
                    let line = format!("{} {} {}", e.origin, e.method, params);
                    let r = ui.add(
                        egui::Label::new(RichText::new(&line).monospace().small().color(color))
                            .sense(egui::Sense::click()),
                    );
                    let r = match &e.error {
                        Some(err) => r.on_hover_text(err),
                        None => r.on_hover_text("click: copy as a vidcut command"),
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

    /// Asks for a project file (on a helper thread: portal dialogs block).
    fn pick_project(&self, new: bool) {
        let live = self.live.clone();
        std::thread::spawn(move || {
            let dialog = rfd::FileDialog::new().add_filter("vidcut project", &["vidcut"]);
            let path = if new {
                dialog.set_file_name("untitled.vidcut").save_file()
            } else {
                dialog.pick_file()
            };
            if let Some(mut path) = path {
                if new && path.extension().is_none() {
                    path.set_extension("vidcut");
                }
                live.request("project.open", json!({ "path": path, "create": new }));
            }
        });
    }

    // ---------------------------------------------------------------- keys

    /// Keyboard shortcuts (spec §11). Ignored while a text field has focus.
    fn keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
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

        // Transport; in Dynamic Trim mode J/K/L and Space trim instead (p. 731).
        let dynamic = self.timeline.dynamic.is_some();
        if dynamic {
            self.dynamic_keys(&input);
        }
        if pressed(Key::Space) && !dynamic {
            self.send("player.toggle", json!({}));
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
                    // Repeated J/L doubles the speed (p. 696).
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
            // Dynamic Trim moves along the track of its edit (p. 732), on
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
                // Dynamic Trim moves on to that edit (p. 732).
                if dynamic && !m.shift {
                    self.dynamic_commit();
                    let (side, track) = self.timeline.edit.map_or(
                        (
                            crate::gui::timeline::Side::Both,
                            crate::gui::timeline::TrackRef::MAIN,
                        ),
                        |e| (e.side, e.track),
                    );
                    self.timeline.edit = Some(crate::gui::timeline::EditSel { at: f, side, track });
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

        // Marks (p. 691). Out marks the frame under the playhead as the last
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
        if pressed(Key::Backslash) && m.command {
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
                // M on a marker edits it; elsewhere it adds one (p. 718).
                let at = self.snap.timeline.position;
                let existing = self
                    .project()
                    .and_then(|p| p.markers.iter().find(|k| k.at == at))
                    .map(|k| k.id.clone());
                match (existing, self.project().cloned()) {
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

        // Timecode entry: `=` absolute, `+`/`-` relative (p. 688).
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

    /// Edit points to jump between with Up/Down (p. 640).
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
    /// is over a pool thumbnail, marks that clip there (p. 691).
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

    /// Delete: the timeline In–Out range if set (p. 725), else the selected
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
        self.keys(&ctx);
        self.export_windows(&ctx);
        self.preferences_window(&ctx);
        self.import_window(&ctx);
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
