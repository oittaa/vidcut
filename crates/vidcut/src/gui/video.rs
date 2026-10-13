//! The viewer's picture (docs/decisions/preview.md §4): a thread of its
//! own presents the player's engine, drawing the picture its clock is at
//! with the GPU shader ([`super::picture`]) into a target the size of the
//! viewer's picture in physical pixels, and answers the session's presents
//! and screenshots. egui shows the target as a native texture.
//!
//! Not on the UI thread: while the window is hidden, presenting the window
//! may block that thread for as long as it stays hidden (NVIDIA on
//! Wayland), and a seek must still show its frame and the viewer be
//! captured. The viewer thread's submissions to the same device go on
//! meanwhile.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use eframe::egui;
use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use vidcut_av::player::Player;
use vidcut_core::Error;

use super::picture::{Drawn, Gpu, Screen};
use crate::live::{Capture, Presented};

type Engine = Arc<Player>;

/// What the session asks of the viewer thread.
enum Task {
    Present(Presented),
    Capture(Capture),
}

/// The viewer's target as the UI shows it: counted anew with each new
/// target.
#[derive(Clone)]
struct Target {
    generation: u64,
    view: wgpu::TextureView,
}

#[derive(Default)]
struct State {
    tasks: Vec<Task>,
    engine: Option<Engine>,
    /// The size (physical pixels) the viewer shows the picture at.
    want: [i32; 2],
    target: Option<Target>,
    /// The target holds a picture (not black for want of one).
    has_frame: bool,
    woken: bool,
    stop: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn change(&self, f: impl FnOnce(&mut State)) {
        let mut s = self.lock();
        f(&mut s);
        s.woken = true;
        drop(s);
        self.wake.notify_all();
    }
}

/// The viewer thread, and what the UI shows of it.
pub struct Viewer {
    shared: Arc<Shared>,
    /// It shows pictures in GPU memory (CUDA's, VA-API's) from there.
    gpu_pictures: bool,
    thread: Option<JoinHandle<()>>,
    /// The egui texture of the target, and the target's generation.
    texture: Option<(u64, egui::TextureId)>,
}

/// Asks things of the viewer thread from anywhere (the session's side).
#[derive(Clone)]
pub struct ViewerHandle {
    shared: Arc<Shared>,
}

/// When the viewer thread looks again without being woken.
const IDLE: Duration = Duration::from_millis(100);

impl Viewer {
    /// Starts the viewer thread on eframe's device; `repaint` asks the UI
    /// to show a changed picture.
    pub fn start(render: &RenderState, repaint: impl Fn() + Send + 'static) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                want: [640, 360],
                ..State::default()
            }),
            wake: Condvar::new(),
        });
        let gpu = Gpu::new(&render.device, &render.queue);
        // Pictures in the memory of the GPU the player decodes on.
        let gpu_pictures =
            vidcut_av::hw::GpuDecoder::best().is_some_and(|d| gpu.takes_gpu_pictures(d.kind()));
        let s = shared.clone();
        let thread = crate::threads::spawn(
            "vidcut-viewer",
            crate::threads::Work::Interactive,
            move || run(&s, &gpu, &repaint),
        )
        .ok();
        Self {
            shared,
            gpu_pictures,
            thread,
            texture: None,
        }
    }

    /// Whether it shows the pictures the player's GPU decodes (NVDEC's,
    /// VA-API's) from GPU memory, so the player may keep them there.
    pub fn takes_gpu_pictures(&self) -> bool {
        self.gpu_pictures
    }

    pub fn handle(&self) -> ViewerHandle {
        ViewerHandle {
            shared: self.shared.clone(),
        }
    }
}

impl ViewerHandle {
    /// The engine the viewer shows from now on.
    pub fn attach(&self, engine: Option<Engine>) {
        self.shared.change(|s| s.engine = engine);
    }

    /// Draw the picture now and say which frame it showed.
    pub fn present(&self, done: Presented) {
        self.shared.change(|s| s.tasks.push(Task::Present(done)));
    }

    /// The picture the viewer shows, as drawn.
    pub fn capture(&self, done: Capture) {
        self.shared.change(|s| s.tasks.push(Task::Capture(done)));
    }

    /// Looks again (a picture landed, or the session changed something).
    pub fn wake(&self) {
        self.shared.change(|_| {});
    }
}

impl Viewer {
    /// The UI's side, each frame: the size the viewer shows the picture
    /// at; the picture's egui texture (registered anew with each new
    /// target) and whether it holds a picture.
    pub fn sync(
        &mut self,
        render: &RenderState,
        want: [i32; 2],
    ) -> (Option<egui::TextureId>, bool) {
        let (target, has_frame) = {
            let mut s = self.shared.lock();
            let changed = s.want != want;
            s.want = want;
            if changed {
                s.woken = true;
                self.shared.wake.notify_all();
            }
            (s.target.clone(), s.has_frame)
        };
        if let Some(t) = &target {
            // Drawn one to one: the viewer's picture rectangle is the
            // target's size in physical pixels.
            let filter = wgpu::FilterMode::Nearest;
            match self.texture {
                Some((g, _)) if g == t.generation => {}
                Some((_, tex)) => {
                    render
                        .renderer
                        .write()
                        .update_egui_texture_from_wgpu_texture(
                            &render.device,
                            &t.view,
                            filter,
                            tex,
                        );
                    self.texture = Some((t.generation, tex));
                }
                None => {
                    let tex = render.renderer.write().register_native_texture(
                        &render.device,
                        &t.view,
                        filter,
                    );
                    self.texture = Some((t.generation, tex));
                }
            }
        }
        (self.texture.map(|(_, t)| t), has_frame)
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.shared.change(|s| {
            s.stop = true;
            s.engine = None;
        });
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The viewer thread: presents the engine whenever woken, and as its clock
/// moves while it plays.
fn run(shared: &Shared, gpu: &Gpu, repaint: &dyn Fn()) {
    let mut screen = Screen::new();
    // Counts new targets: the UI registers each anew.
    let mut generation = 0u64;
    let mut next: Option<Instant> = None;
    loop {
        let (tasks, engine, want) = {
            let mut s = shared.lock();
            while !s.woken && !s.stop {
                let wait = next.map_or(IDLE, |n| n.saturating_duration_since(Instant::now()));
                if wait.is_zero() {
                    break;
                }
                s = shared
                    .wake
                    .wait_timeout(s, wait)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
            if s.stop {
                return;
            }
            s.woken = false;
            (std::mem::take(&mut s.tasks), s.engine.clone(), s.want)
        };
        let now = Instant::now();
        let shown = engine.as_ref().and_then(|e| e.present(now));
        let [w, h] = want;
        let drawn = screen.draw(
            gpu,
            shown.as_ref().map(|s| &s.picture),
            w.max(2) as u32,
            h.max(2) as u32,
        );
        let changed = drawn != Drawn::Same;
        if changed && crate::live::debug() {
            eprintln!(
                "[viewer] drew {:?} at {w}x{h}",
                shown.as_ref().map(|s| (s.frame, s.exact))
            );
        }
        if drawn == Drawn::Target {
            generation += 1;
        }
        let target = screen.view().map(|view| Target {
            generation,
            view: view.clone(),
        });
        let has_frame = shown.is_some();
        let presented = shown.filter(|s| s.exact).map(|s| s.frame);
        // Playing: look again when the clock moves to the next frame.
        next = engine
            .as_ref()
            .and_then(|e| e.next_change(now))
            .map(|d| now + d.max(Duration::from_millis(1)));
        {
            let mut s = shared.lock();
            s.target = target;
            s.has_frame = has_frame;
        }
        for task in tasks {
            match task {
                Task::Present(done) => done(presented),
                Task::Capture(done) => {
                    let image = screen.read(gpu);
                    done(image.ok_or_else(|| {
                        Error::new(
                            vidcut_core::ErrorCode::Internal,
                            "the viewer has no picture",
                        )
                    }));
                }
            }
        }
        if changed {
            repaint();
        }
    }
}
