//! The viewers' pictures (docs/decisions/preview.md §4): a thread of their
//! own presents both players' engines, drawing the picture each one's
//! clock is at with the GPU shader ([`super::picture`]) into a target the
//! size of the viewer's picture in physical pixels, and answers the
//! session's presents and screenshots. egui shows the targets as native
//! textures.
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
use crate::live::{Capture, PlayerId, Presented};

type Engine = Arc<Player>;

/// What the session asks of the viewer thread.
enum Task {
    Present(PlayerId, Presented),
    Capture(PlayerId, Capture),
}

/// A viewer's target as the UI shows it: counted anew with each new target.
#[derive(Clone)]
struct Target {
    generation: u64,
    view: wgpu::TextureView,
}

#[derive(Default)]
struct State {
    tasks: Vec<Task>,
    engines: [Option<Engine>; 2],
    /// The sizes (physical pixels) the viewer shows each player's picture
    /// at.
    wants: [[i32; 2]; 2],
    targets: [Option<Target>; 2],
    /// A target holds a picture (not black for want of one).
    has_frame: [bool; 2],
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
    thread: Option<JoinHandle<()>>,
    /// The egui texture of each target, and the target's generation.
    textures: [Option<(u64, egui::TextureId)>; 2],
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
                wants: [[640, 360]; 2],
                ..State::default()
            }),
            wake: Condvar::new(),
        });
        let gpu = Gpu::new(&render.device, &render.queue);
        let s = shared.clone();
        let thread = crate::threads::spawn(
            "vidcut-viewer",
            crate::threads::Work::Interactive,
            move || run(&s, &gpu, &repaint),
        )
        .ok();
        Self {
            shared,
            thread,
            textures: [None, None],
        }
    }

    pub fn handle(&self) -> ViewerHandle {
        ViewerHandle {
            shared: self.shared.clone(),
        }
    }
}

impl ViewerHandle {
    /// The engine `player` shows from now on.
    pub fn attach(&self, player: PlayerId, engine: Option<Engine>) {
        self.shared.change(|s| s.engines[player.index()] = engine);
    }

    /// Draw `player`'s picture now and say which frame it showed.
    pub fn present(&self, player: PlayerId, done: Presented) {
        self.shared
            .change(|s| s.tasks.push(Task::Present(player, done)));
    }

    /// The picture `player`'s viewer shows, as drawn.
    pub fn capture(&self, player: PlayerId, done: Capture) {
        self.shared
            .change(|s| s.tasks.push(Task::Capture(player, done)));
    }

    /// Looks again (a picture landed, or the session changed something).
    pub fn wake(&self) {
        self.shared.change(|_| {});
    }
}

impl Viewer {
    /// The UI's side, each frame: the sizes the viewer shows the pictures
    /// at, and for each player its picture's egui texture (registered
    /// anew with each new target) and whether it holds a picture.
    pub fn sync(
        &mut self,
        render: &RenderState,
        wants: [[i32; 2]; 2],
    ) -> [(Option<egui::TextureId>, bool); 2] {
        let (targets, has_frame) = {
            let mut s = self.shared.lock();
            let changed = s.wants != wants;
            s.wants = wants;
            if changed {
                s.woken = true;
                self.shared.wake.notify_all();
            }
            (s.targets.clone(), s.has_frame)
        };
        let mut out = [(None, false); 2];
        for id in PlayerId::ALL {
            let i = id.index();
            if let Some(t) = &targets[i] {
                // Drawn one to one: the viewer's picture rectangle is the
                // target's size in physical pixels.
                let filter = wgpu::FilterMode::Nearest;
                match self.textures[i] {
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
                        self.textures[i] = Some((t.generation, tex));
                    }
                    None => {
                        let tex = render.renderer.write().register_native_texture(
                            &render.device,
                            &t.view,
                            filter,
                        );
                        self.textures[i] = Some((t.generation, tex));
                    }
                }
            }
            out[i] = (self.textures[i].map(|(_, t)| t), has_frame[i]);
        }
        out
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.shared.change(|s| {
            s.stop = true;
            s.engines = [None, None];
        });
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The viewer thread: presents both engines whenever woken, and as the
/// clocks of those playing move.
fn run(shared: &Shared, gpu: &Gpu, repaint: &dyn Fn()) {
    let mut screens = [Screen::new(), Screen::new()];
    // Counts each player's new targets: the UI registers each anew.
    let mut generations = [0u64; 2];
    let mut next: Option<Instant> = None;
    loop {
        let (tasks, engines, wants) = {
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
            (std::mem::take(&mut s.tasks), s.engines.clone(), s.wants)
        };
        let now = Instant::now();
        let mut presented = [None; 2];
        let mut changed = false;
        let mut targets = [None, None];
        let mut has_frame = [false; 2];
        next = None;
        for id in PlayerId::ALL {
            let i = id.index();
            let engine = engines[i].as_deref();
            let shown = engine.and_then(|e| e.present(now));
            let [w, h] = wants[i];
            let drawn = screens[i].draw(
                gpu,
                shown.as_ref().map(|s| &s.picture),
                w.max(2) as u32,
                h.max(2) as u32,
            );
            if drawn != Drawn::Same {
                changed = true;
                if crate::live::debug() {
                    eprintln!(
                        "[viewer] {id:?} drew {:?} at {w}x{h}",
                        shown.as_ref().map(|s| (s.frame, s.exact))
                    );
                }
            }
            if drawn == Drawn::Target {
                generations[i] += 1;
            }
            targets[i] = screens[i].view().map(|view| Target {
                generation: generations[i],
                view: view.clone(),
            });
            has_frame[i] = shown.is_some();
            presented[i] = shown.filter(|s| s.exact).map(|s| s.frame);
            // Playing: look again when the clock moves to the next frame.
            if let Some(d) = engine.and_then(|e| e.next_change(now)) {
                let at = now + d.max(Duration::from_millis(1));
                next = Some(next.map_or(at, |n| n.min(at)));
            }
        }
        {
            let mut s = shared.lock();
            s.targets = targets;
            s.has_frame = has_frame;
        }
        for task in tasks {
            match task {
                Task::Present(id, done) => done(presented[id.index()]),
                Task::Capture(id, done) => {
                    let image = screens[id.index()].read(gpu);
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
