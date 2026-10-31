use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use eframe::egui_wgpu::RenderState;
use eframe::wgpu::rwh::HasWindowHandle;

use super::theme;
use super::video::{Viewer, ViewerHandle};
use super::workspace::{Picture, Workspace};
use crate::live::{self, Capture, Image, Presented};
use vidcut_av::screen::Gate;

enum UiTask {
    CaptureWindow(Capture),
    Window(live::WindowChange),
    Gui(Vec<live::GuiChange>, live::GuiDone),
    TrimEditor(live::TrimEditor),
    DynamicTrim(live::DynamicTrim),
    Quit,
}

pub struct Port {
    ctx: egui::Context,
    viewer: ViewerHandle,
    tasks: Mutex<Vec<UiTask>>,
    session: std::sync::OnceLock<live::Handle>,
    two_up: Mutex<live::stats::TwoUpStats>,
    gui: Mutex<Option<live::GuiState>>,
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

    fn viewer_stats(&self) -> Option<live::stats::ViewerStats> {
        Some(self.viewer.stats())
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
        if let Some(session) = self.session.get().cloned() {
            let _ = crate::threads::spawn(
                "vidcut-quit",
                crate::threads::Work::Interactive,
                move || {
                    std::thread::sleep(QUIT_PATIENCE);
                    eprintln!("vidcut: the window didn't close; quitting without it");
                    session.shutdown_and_wait(Duration::from_secs(5));
                    crate::threads::join_closing();
                    crate::threads::exit_now(0);
                },
            );
        }
    }

    fn gui(&self, changes: Vec<live::GuiChange>, done: live::GuiDone) {
        self.push(UiTask::Gui(changes, done));
    }

    fn gui_state(&self) -> Option<live::GuiState> {
        self.gui.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn trim_editor(&self, r: live::TrimEditor) {
        self.push(UiTask::TrimEditor(r));
    }

    fn dynamic_trim(&self, r: live::DynamicTrim) {
        self.push(UiTask::DynamicTrim(r));
    }

    fn two_up(&self) -> Option<live::stats::TwoUpStats> {
        Some(
            self.two_up
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        )
    }
}

pub struct App {
    ws: Workspace,
    port: Arc<Port>,
    render: RenderState,
    viewer: Viewer,
    gate: Arc<Gate>,
    session: Option<std::thread::JoinHandle<()>>,
    window: super::window::Window,
}

const QUIT_PATIENCE: Duration = Duration::from_secs(5);

const EXIT_PATIENCE: Duration = Duration::from_secs(10);

fn audio_device() -> Box<dyn vidcut_av::player::AudioOutput> {
    Box::new(vidcut_av::player::CpalOutput::on(
        crate::preferences::audio_device(),
    ))
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        project: Option<PathBuf>,
        window: super::window::Window,
    ) -> std::result::Result<Self, String> {
        let render = cc
            .wgpu_render_state
            .clone()
            .ok_or("vidcut needs eframe's wgpu renderer")?;
        let ctx = cc.egui_ctx.clone();
        let gate = Arc::new(Gate::new(move || ctx.request_repaint()));
        let ctx = cc.egui_ctx.clone();
        let viewer = Viewer::start(&render, gate.clone(), move || ctx.request_repaint());
        let port = Arc::new(Port {
            ctx: cc.egui_ctx.clone(),
            viewer: viewer.handle(),
            tasks: Mutex::new(Vec::new()),
            session: std::sync::OnceLock::new(),
            two_up: Mutex::default(),
            gui: Mutex::default(),
        });
        let socket = crate::ipc::registry::socket_path(std::process::id());
        let create = project.as_ref().is_some_and(|p| !p.exists());
        let notify = {
            let (ctx, viewer) = (cc.egui_ctx.clone(), viewer.handle());
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
                gpu_pictures: viewer.gpu_pictures(),
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
        ws.screens = Some(super::cells::Screens::new(&render));
        let info = render.adapter.get_info();
        ws.display = Some(format!("{} ({:?})", info.name, info.backend));
        Ok(Self {
            ws,
            port,
            render,
            viewer,
            gate,
            session: Some(session),
            window,
        })
    }

    fn publish(&self) {
        *self.port.gui.lock().unwrap_or_else(|e| e.into_inner()) = Some(self.ws.gui_state());
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let gate = self.gate.clone();
        let _open = gate.opened();
        for task in self.port.take() {
            match task {
                UiTask::CaptureWindow(done) => {
                    ctx.request_screenshot(move |image| {
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
                    });
                }
                UiTask::Window(w) => {
                    use egui::ViewportCommand as C;
                    if let Some([width, height]) = w.size {
                        ctx.send_viewport_cmd(C::InnerSize(egui::vec2(width, height)));
                    }
                    if let Some([x, y]) = w.position {
                        ctx.send_viewport_cmd(C::OuterPosition(egui::pos2(x, y)));
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
                UiTask::Gui(changes, done) => {
                    let made = self.ws.gui_change(changes, ctx);
                    self.publish();
                    done(made.map(|()| self.ws.gui_state()));
                }
                UiTask::TrimEditor(r) => self.ws.trim_editor_request(r),
                UiTask::DynamicTrim(r) => self.ws.dynamic_trim_request(r, ctx.input(|i| i.time)),
                UiTask::Quit => {
                    self.ws.export.quit_confirmed = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let gate = self.gate.clone();
        let _open = gate.opened();
        let (texture, has_frame) =
            self.viewer
                .sync(&self.render, self.ws.want, self.ws.snap.show_super_scale);
        self.ws.picture = Picture { texture, has_frame };
        self.ws.levels = self.viewer.levels();
        let ctx = ui.ctx().clone();
        self.ws.stats = self.ws.snap.stats.then(|| self.viewer.stats()).flatten();
        if self.ws.stats.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        self.window.watch(&ctx, frame);
        self.ws.main_window = frame.window_handle().ok().map(|h| h.as_raw());
        self.ws.ui(ui);
        *self.port.two_up.lock().unwrap_or_else(|e| e.into_inner()) = self.ws.two_up_stats();
        self.publish();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let [r, g, b, a] = theme::colors().background.to_normalized_gamma_f32();
        [r, g, b, a]
    }

    fn on_exit(&mut self) {
        self.window.flush();
        self.ws.flush_windows();
        crate::threads::end_within(EXIT_PATIENCE);
        crate::threads::ending("the session");
        self.gate.open();
        let started = std::time::Instant::now();
        self.ws.live.shutdown();
        if let Some(t) = self.session.take() {
            let _ = t.join();
        }
        vidcut_av::debug!(
            "gui",
            "the session ended in {:.0} ms",
            started.elapsed().as_secs_f64() * 1000.0
        );
        crate::threads::ending("the window and its app");
    }
}
