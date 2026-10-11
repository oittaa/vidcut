//! The eframe app: OpenGL video, captures and window commands around the
//! [`Workspace`]. The UI thread is also mpv's render thread, so it only
//! renders frames and never calls mpv's client API
//! (docs/decisions/preview.md §6).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui;
use eframe::glow;
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use vidcut_core::Error;
use vidcut_core::request::GuiPanel;
use vidcut_mpv::{Display, GlRenderer, Mpv};

use super::theme;
use super::video::Video;
use super::workspace::{Picture, Workspace};
use crate::live::{self, Capture, Image, PlayerId};

/// Work the session asks of the UI thread.
enum UiTask {
    CaptureViewer(PlayerId, Capture),
    CaptureWindow(Capture),
    Window(live::WindowChange),
    Show(GuiPanel),
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

    fn show(&self, panel: GuiPanel) {
        self.push(UiTask::Show(panel));
    }
}

pub struct App {
    ws: Workspace,
    port: Arc<Port>,
    gl: Arc<glow::Context>,
    videos: [Video; 2],
    session: Option<std::thread::JoinHandle<()>>,
    window_captures: HashMap<u64, Capture>,
    next_capture: u64,
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
            ws: Workspace::new(handle),
            port,
            gl,
            videos: [Video::new(s), Video::new(t)],
            session: Some(session),
            window_captures: HashMap::new(),
            next_capture: 1,
        })
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
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Runs even while the window is hidden: keep rendering so mpv never
        // waits for us and the viewer can be captured.
        let gl = self.gl.clone();
        for id in PlayerId::ALL {
            if let Some(new) = self.videos[id.index()].render(&gl, frame) {
                self.ws.live.rendered(id, new);
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
                UiTask::Show(panel) => self.ws.show_panel(panel, ctx),
                UiTask::Quit => {
                    // `gui.quit` doesn't ask about running exports.
                    self.ws.export.quit_confirmed = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        for id in PlayerId::ALL {
            let v = &self.videos[id.index()];
            self.ws.pictures[id.index()] = Picture {
                texture: v.texture_id,
                has_frame: v.has_frame,
            };
        }
        let ctx = ui.ctx().clone();
        self.window_screenshots(&ctx);
        self.ws.ui(ui);
        for id in PlayerId::ALL {
            let want = self.ws.wants[id.index()];
            if self.videos[id.index()].want != want {
                // The next logic() pass resizes the target and redraws.
                self.videos[id.index()].want = want;
                ctx.request_repaint();
            }
        }
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
        self.ws.live.shutdown();
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
    let make = |id: PlayerId| -> std::result::Result<(Mpv, GlRenderer), String> {
        let mut opts = live::mpv_options(true);
        opts.extend(live::mpv_log_options(id));
        let opts: Vec<(&str, &str)> = opts.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let mpv = Mpv::new(&opts).map_err(|e| e.to_string())?;
        let l = lookup.clone();
        let renderer = GlRenderer::new(&mpv, Box::new(move |name| l(name)), display)
            .map_err(|e| e.to_string())?;
        let ctx = ctx.clone();
        renderer.set_update_callback(move || ctx.request_repaint());
        Ok((mpv, renderer))
    };
    // The timeline player comes first: FFmpeg has one log callback per
    // process, which the first mpv takes, so all of FFmpeg's messages go to
    // the timeline's log (`VIDCUT_MPV_LOG_DIR`).
    let (t, tr) = make(PlayerId::Timeline)?;
    let (s, sr) = make(PlayerId::Source)?;
    Ok(((s, t), [sr, tr]))
}
