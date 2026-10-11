//! The eframe app: layout, input and rendering on the UI thread.
//!
//! The UI is a thin view. Each frame it reads the session's latest
//! [`Snapshot`] and draws it; every user action becomes one [`Request`] sent to
//! the session (AGENTS.md rule 2), the same requests the CLI and IPC clients
//! send. The UI thread is also mpv's render thread, so it only renders frames
//! and never calls mpv's client API (docs/decisions/preview.md §6).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Key, RichText};
use eframe::glow;
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use serde_json::{Value, json};
use vidcut_core::Error;
use vidcut_core::model::Project;
use vidcut_core::request::ViewerMode;
use vidcut_core::time::{Rational, format_timecode};
use vidcut_mpv::{Display, GlRenderer, Mpv};

use super::theme;
use super::video::Video;
use crate::live::{self, Capture, Filmstrip, Image, PlayerId, Snapshot};

/// Work the session asks of the UI thread.
enum UiTask {
    CaptureViewer(PlayerId, Capture),
    CaptureWindow(Capture),
    Window(live::WindowChange),
    Quit,
}

/// The session's view of the GUI ([`live::Renderer`]).
pub struct Port {
    ctx: egui::Context,
    tasks: Mutex<Vec<UiTask>>,
}

impl Port {
    fn push(&self, task: UiTask) {
        self.tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(task);
        self.ctx.request_repaint();
    }

    fn take(&self) -> Vec<UiTask> {
        std::mem::take(&mut *self.tasks.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl live::Renderer for Port {
    fn capture_viewer(&self, player: PlayerId, done: Capture) {
        self.push(UiTask::CaptureViewer(player, done));
    }

    fn capture_window(&self, done: Capture) {
        self.push(UiTask::CaptureWindow(done));
    }

    fn window(&self, change: live::WindowChange) {
        self.push(UiTask::Window(change));
    }

    fn quit(&self) {
        self.push(UiTask::Quit);
    }
}

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

pub struct App {
    pub(super) live: live::Handle,
    port: Arc<Port>,
    gl: Arc<glow::Context>,
    pub(super) videos: [Video; 2],
    pub(super) snap: Arc<Snapshot>,
    session: Option<std::thread::JoinHandle<()>>,
    pub(super) thumbs: HashMap<String, Thumbs>,
    window_captures: HashMap<u64, Capture>,
    next_capture: u64,
    // View state (not part of the project).
    pub(super) pool_view: PoolView,
    pub(super) search: String,
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
    /// What a press in the viewer's scroll bar is dragging.
    pub(super) drag: Option<Drag>,
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
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        project: Option<PathBuf>,
    ) -> std::result::Result<Self, String> {
        theme::apply(&cc.egui_ctx);
        let gl = cc
            .gl
            .clone()
            .ok_or("vidcut needs eframe's OpenGL (glow) renderer")?;
        let display = match cc.display_handle().map(|h| h.as_raw()) {
            Ok(RawDisplayHandle::Wayland(h)) => Display::Wayland(h.display.as_ptr()),
            Ok(RawDisplayHandle::Xlib(h)) => {
                Display::X11(h.display.map_or(std::ptr::null_mut(), |d| d.as_ptr()))
            }
            _ => Display::None,
        };
        let (players, renderers, preview_error) =
            match make_players(&cc.egui_ctx, cc.get_proc_address.clone(), display) {
                Ok((players, [s, t])) => (Some(players), [Some(s), Some(t)], None),
                Err(e) => (None, [None, None], Some(e)),
            };
        let port = Arc::new(Port {
            ctx: cc.egui_ctx.clone(),
            tasks: Mutex::new(Vec::new()),
        });
        let socket = crate::ipc::registry::socket_path(std::process::id());
        let ctx = cc.egui_ctx.clone();
        let create = project.as_ref().is_some_and(|p| !p.exists());
        let (handle, session) = live::start(live::Config {
            project,
            create,
            players,
            preview_error,
            renderer: port.clone(),
            notify: Arc::new(move || ctx.request_repaint()),
            socket: Some(socket.clone()),
        });
        let ipc = handle.clone();
        let served = crate::ipc::server::start(
            &socket,
            Arc::new(move |envelope, project, progress| {
                ipc.call(envelope, live::Origin::Ipc, project, progress)
            }),
        );
        if let Err(e) = served {
            eprintln!(
                "vidcut: live control is off: can't listen on {}: {e}",
                socket.display()
            );
        }
        let [s, t] = renderers;
        Ok(Self {
            snap: handle.snapshot(),
            live: handle,
            port,
            gl,
            videos: [Video::new(s), Video::new(t)],
            session: Some(session),
            thumbs: HashMap::new(),
            window_captures: HashMap::new(),
            next_capture: 1,
            pool_view: PoolView::Thumbnails,
            search: String::new(),
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
            drag: None,
        })
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

    /// Source ranges for an edit function (spec §7): the clip in the source
    /// viewer with its marks, or in Timeline mode the media selected in the
    /// pool. Ranges are explicit so the command log replays exactly.
    pub(super) fn edit_sources(&self) -> Vec<Value> {
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
                "edit.overwrite" | "edit.ripple_overwrite" => s["at"] = json!(playhead),
                _ => {}
            }
            s
        };
        let video_only = self.video_only;
        let sources: Vec<Value> = sources
            .into_iter()
            .map(|mut s| {
                if video_only {
                    s["video_only"] = json!(true);
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

    fn window_screenshots(&mut self, ctx: &egui::Context) {
        let shots: Vec<(u64, Arc<egui::ColorImage>)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Screenshot {
                        user_data, image, ..
                    } => user_data
                        .data
                        .as_ref()
                        .and_then(|d| d.downcast_ref::<u64>())
                        .map(|id| (*id, image.clone())),
                    _ => None,
                })
                .collect()
        });
        for (id, image) in shots {
            if let Some(done) = self.window_captures.remove(&id) {
                let rgba = image
                    .pixels
                    .iter()
                    .flat_map(|c| [c.r(), c.g(), c.b(), 255])
                    .collect();
                done(Ok(Image {
                    width: image.size[0] as u32,
                    height: image.size[1] as u32,
                    rgba,
                }));
            }
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
                if ui
                    .add_enabled(self.project().is_some(), egui::Button::new("Import Media…"))
                    .clicked()
                {
                    self.import_media();
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
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.toggle_value(&mut self.show_log, "Command log");
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

    /// Ctrl+I: pick media files to import (p. 654).
    pub(super) fn import_media(&self) {
        if self.project().is_none() {
            return;
        }
        let live = self.live.clone();
        std::thread::spawn(move || {
            if let Some(files) = rfd::FileDialog::new()
                .add_filter(
                    "media",
                    &[
                        "mp4", "mov", "mkv", "webm", "avi", "mxf", "m4v", "ts", "mts", "m2ts",
                        "mpg", "wav", "flac", "mp3", "m4a", "aac", "ogg", "opus",
                    ],
                )
                .add_filter("all files", &["*"])
                .pick_files()
            {
                live.request("media.add", json!({ "paths": files }));
            }
        });
    }

    // ---------------------------------------------------------------- keys

    /// Keyboard shortcuts (spec §11). Ignored while a text field has focus.
    fn keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() || self.snap.project.is_none() {
            return;
        }
        let input = ctx.input(|i| i.clone());
        let m = input.modifiers;
        let pressed = |k: Key| input.key_pressed(k);
        let viewer = self.snap.viewer;
        let id = PlayerId::of(viewer);
        let player = self.snap.player(id).clone();
        let fps = self.fps().as_f64().round().max(1.0) as i64;

        // Clipboard: egui turns Ctrl+C/X/V into events.
        for e in &input.events {
            match e {
                egui::Event::Copy | egui::Event::Cut if !self.snap.selection.clips.is_empty() => {
                    let clips = self.snap.selection.clips.clone();
                    // Also put the clips on the system clipboard as JSON (paste
                    // them into another project with `edit paste --clips`).
                    if let Some(p) = self.project() {
                        let data: Vec<_> = clips.iter().filter_map(|c| p.clip(c)).collect();
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

        // Transport.
        if pressed(Key::Space) {
            self.send("player.toggle", json!({}));
        }
        if pressed(Key::K) {
            self.send("player.pause", json!({}));
        }
        let k_down = input.key_down(Key::K);
        for (key, dir) in [(Key::L, 1.0), (Key::J, -1.0)] {
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
            let points = if m.shift {
                self.marker_points()
            } else {
                self.edit_points(id)
            };
            let target = if up {
                points.iter().rev().find(|&&f| f < player.position)
            } else {
                points.iter().find(|&&f| f > player.position)
            };
            if let Some(&f) = target {
                self.seek(id, f, false);
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
        if pressed(Key::I) || pressed(Key::O) {
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
        if pressed(Key::F12) && m.shift {
            self.edit_function("edit.append");
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
                self.send("markers.add", json!({ "at": self.snap.timeline.position }));
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
                ViewerMode::SourceTape => self.snap.tape.iter().map(|e| e.start).collect(),
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

    /// I/O: marks the source clip in source modes, the timeline otherwise.
    /// While the pointer is over a pool thumbnail, marks that clip there (p. 691).
    pub(super) fn mark(&self, is_in: bool) {
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
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Runs even while the window is hidden: keep rendering so mpv never
        // waits for us and the viewer can be captured.
        let gl = self.gl.clone();
        for id in PlayerId::ALL {
            if let Some(new) = self.videos[id.index()].render(&gl, frame) {
                self.live.rendered(id, new);
            }
        }
        for task in self.port.take() {
            match task {
                UiTask::CaptureViewer(id, done) => {
                    let image = self.videos[id.index()].read_back(&gl);
                    done(image.ok_or_else(|| {
                        Error::new(
                            vidcut_core::ErrorCode::Dependency,
                            "the viewer has no picture (is libmpv available? see `vidcut doctor`)",
                        )
                    }));
                }
                UiTask::CaptureWindow(done) => {
                    let id = self.next_capture;
                    self.next_capture += 1;
                    self.window_captures.insert(id, done);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
                        id,
                    )));
                }
                UiTask::Window(w) => {
                    use egui::ViewportCommand as C;
                    if let Some([width, height]) = w.size {
                        ctx.send_viewport_cmd(C::InnerSize(egui::vec2(width, height)));
                    }
                    if let Some(m) = w.maximized {
                        ctx.send_viewport_cmd(C::Maximized(m));
                    }
                    if let Some(f) = w.fullscreen {
                        ctx.send_viewport_cmd(C::Fullscreen(f));
                    }
                    if let Some(m) = w.minimized {
                        ctx.send_viewport_cmd(C::Minimized(m));
                    }
                }
                UiTask::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.snap = self.live.snapshot();
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
        self.window_screenshots(&ctx);
        self.keys(&ctx);
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
            .default_size(240.0)
            .min_size(140.0)
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(6))
            .show(ui, |ui| self.timeline_panel(ui));
        egui::Panel::left("pool")
            .resizable(true)
            .default_size(430.0)
            .min_size(260.0)
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(6))
            .show(ui, |ui| self.pool_panel(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(6))
            .show(ui, |ui| self.viewer_panel(ui));
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let [r, g, b, a] = theme::BG.to_normalized_gamma_f32();
        [r, g, b, a]
    }

    fn on_exit(&mut self, gl: Option<&glow::Context>) {
        // Render contexts go first, while the GL context is current; the mpv
        // handles go when the session (and these) drop them.
        for v in &mut self.videos {
            v.destroy(gl);
        }
        self.live.shutdown();
        if let Some(t) = self.session.take() {
            let _ = t.join();
        }
    }
}

/// eframe's OpenGL function loader.
type GlLoader = Arc<dyn Fn(&std::ffi::CStr) -> *const std::ffi::c_void + Send + Sync>;

/// Creates both players and their GL render contexts.
fn make_players(
    ctx: &egui::Context,
    proc_address: Option<GlLoader>,
    display: Display,
) -> std::result::Result<((Mpv, Mpv), [GlRenderer; 2]), String> {
    vidcut_mpv::available()?;
    let lookup = proc_address.ok_or("no OpenGL function loader")?;
    let opts = live::mpv_options(true);
    let opts: Vec<(&str, &str)> = opts.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let make = || -> std::result::Result<(Mpv, GlRenderer), String> {
        let mpv = Mpv::new(&opts).map_err(|e| e.to_string())?;
        let l = lookup.clone();
        let renderer = GlRenderer::new(&mpv, Box::new(move |name| l(name)), display)
            .map_err(|e| e.to_string())?;
        let ctx = ctx.clone();
        renderer.set_update_callback(move || ctx.request_repaint());
        Ok((mpv, renderer))
    };
    let (s, sr) = make()?;
    let (t, tr) = make()?;
    Ok(((s, t), [sr, tr]))
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
