//! The eframe app on wgpu: the [`Workspace`], window commands and window
//! captures. The viewer's picture is presented by a thread of its own
//! ([`Viewer`]); the UI only shows it (docs/decisions/preview.md §4).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use eframe::egui_wgpu::RenderState;
use vidcut_core::request::GuiPanel;

use super::theme;
use super::video::{Viewer, ViewerHandle};
use super::workspace::{Picture, Workspace};
use crate::live::{self, Capture, Image, Presented};

/// Work the session asks of the UI thread.
enum UiTask {
    CaptureWindow(Capture),
    Window(live::WindowChange),
    Show(GuiPanel),
    Quit,
}

/// The session's view of the GUI ([`live::Renderer`]): pictures go to the
/// viewer thread, the rest to the UI thread.
pub struct Port {
    ctx: egui::Context,
    viewer: ViewerHandle,
    tasks: Mutex<Vec<UiTask>>,
    /// The session, once started (for `gui.quit` when the window can't
    /// close).
    session: std::sync::OnceLock<live::Handle>,
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
    fn attach(&self, engine: Option<Arc<vidcut_av::player::Player>>) {
        self.viewer.attach(engine);
    }

    fn present(&self, done: Presented) {
        self.viewer.present(done);
    }

    fn capture_viewer(&self, done: Capture) {
        self.viewer.capture(done);
    }

    fn capture_window(&self, done: Capture) {
        self.push(UiTask::CaptureWindow(done));
    }

    fn window(&self, change: live::WindowChange) {
        self.push(UiTask::Window(change));
    }

    fn quit(&self) {
        self.push(UiTask::Quit);
        // A window that can't paint never gets to close: presenting it can
        // block the UI thread for as long as it is hidden (NVIDIA on
        // Wayland, which has no way to show a minimized window again).
        // Then the session ends and the process with it anyway.
        if let Some(session) = self.session.get().cloned() {
            let _ = crate::threads::spawn(
                "vidcut-quit",
                crate::threads::Work::Interactive,
                move || {
                    std::thread::sleep(QUIT_PATIENCE);
                    eprintln!("vidcut: the window didn't close; quitting without it");
                    session.shutdown_and_wait(Duration::from_secs(5));
                    std::process::exit(0);
                },
            );
        }
    }

    fn show(&self, panel: GuiPanel) {
        self.push(UiTask::Show(panel));
    }
}

pub struct App {
    ws: Workspace,
    port: Arc<Port>,
    render: RenderState,
    viewer: Viewer,
    session: Option<std::thread::JoinHandle<()>>,
    window_captures: HashMap<u64, Capture>,
    next_capture: u64,
}

/// How long `gui.quit` waits for the window to close before the process
/// ends without it.
const QUIT_PATIENCE: Duration = Duration::from_secs(5);

/// The player's sound: the device the preferences name, else the system's
/// default; read whenever the player opens its device.
fn audio_device() -> Box<dyn vidcut_av::player::AudioOutput> {
    Box::new(vidcut_av::player::CpalOutput::on(
        crate::preferences::audio_device(),
    ))
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        project: Option<PathBuf>,
    ) -> std::result::Result<Self, String> {
        theme::apply(&cc.egui_ctx);
        let render = cc
            .wgpu_render_state
            .clone()
            .ok_or("vidcut needs eframe's wgpu renderer")?;
        let ctx = cc.egui_ctx.clone();
        let viewer = Viewer::start(&render, move || ctx.request_repaint());
        let port = Arc::new(Port {
            ctx: cc.egui_ctx.clone(),
            viewer: viewer.handle(),
            tasks: Mutex::new(Vec::new()),
            session: std::sync::OnceLock::new(),
        });
        let socket = crate::ipc::registry::socket_path(std::process::id());
        let create = project.as_ref().is_some_and(|p| !p.exists());
        let notify = {
            let (ctx, viewer) = (cc.egui_ctx.clone(), viewer.handle());
            // A new snapshot, or a picture landed: the UI repaints, the
            // viewer looks again.
            Arc::new(move || {
                ctx.request_repaint();
                viewer.wake();
            })
        };
        let (handle, session) = live::start(live::Config {
            project,
            create,
            player: Some(live::PlayerConfig {
                audio: Arc::new(audio_device),
                budget: crate::preferences::player_budget(),
                gpu_pictures: viewer.takes_gpu_pictures(),
            }),
            preview_error: None,
            renderer: port.clone(),
            notify,
            socket: Some(socket.clone()),
        });
        let _ = port.session.set(handle.clone());
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
        let mut ws = Workspace::new(handle);
        let info = render.adapter.get_info();
        ws.display = Some(format!("{} ({:?})", info.name, info.backend));
        Ok(Self {
            ws,
            port,
            render,
            viewer,
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
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        for task in self.port.take() {
            match task {
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
        let (texture, has_frame) = self.viewer.sync(&self.render, self.ws.want);
        self.ws.picture = Picture { texture, has_frame };
        let ctx = ui.ctx().clone();
        self.window_screenshots(&ctx);
        self.ws.ui(ui);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let [r, g, b, a] = theme::BG.to_normalized_gamma_f32();
        [r, g, b, a]
    }

    fn on_exit(&mut self) {
        self.ws.live.shutdown();
        if let Some(t) = self.session.take() {
            let _ = t.join();
        }
    }
}
