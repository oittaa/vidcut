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
use crate::live::stats::{PlayerStats, ViewerStats};
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
    /// What the viewer tells of itself, for stats for nerds.
    stats: ViewerStats,
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
    /// The decoders whose pictures it shows from GPU memory.
    gpu_pictures: Vec<vidcut_av::hw::Kind>,
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
        let gpu_pictures = gpu.gpu_pictures();
        let info = render.adapter.get_info();
        let renderer = format!(
            "wgpu on {:?}, {} ({:?}, driver {} {})",
            info.backend, info.name, info.device_type, info.driver, info.driver_info
        );
        vidcut_av::debug!(
            "viewer",
            "draws with {renderer}; shows {gpu_pictures:?} pictures from GPU memory, \
             uploads the rest"
        );
        {
            let mut s = shared.lock();
            s.stats.backend = backend_name(info.backend);
            s.stats.gpu = info.name.clone();
            s.stats.driver = format!("{} {}", info.driver, info.driver_info)
                .trim()
                .to_string();
        }
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

    /// The decoders whose pictures it shows from GPU memory, so the player
    /// may keep them there.
    pub fn gpu_pictures(&self) -> Vec<vidcut_av::hw::Kind> {
        self.gpu_pictures.clone()
    }

    /// What the engine shown does, with what the viewer tells of itself
    /// (stats for nerds); `None` while it shows none.
    pub fn stats(&self) -> Option<PlayerStats> {
        let (engine, viewer) = {
            let s = self.shared.lock();
            (s.engine.clone()?, s.stats.clone())
        };
        Some(PlayerStats::of(&engine, Some(viewer)))
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

    /// What the viewer tells of itself, for stats for nerds.
    pub fn stats(&self) -> ViewerStats {
        self.shared.lock().stats.clone()
    }
}

impl Viewer {
    /// The peaks of the sound the engine shown plays now, while it plays.
    pub fn levels(&self) -> Option<vidcut_av::player::Levels> {
        let engine = self.shared.lock().engine.clone()?;
        engine.levels(Instant::now())
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
    /// The window closes: the viewer thread ends, and with it the screen,
    /// whose buffers are the display's device's. The engine shown ends on
    /// a thread of its own (`vidcut-viewer-close`), as the two-up's
    /// players do: it holds nothing of the display's device, its pictures
    /// being its decoder's frames, and freeing their GPU memory waits its
    /// turn behind the two-up's players ending. The UI doesn't wait for
    /// that.
    fn drop(&mut self) {
        let started = Instant::now();
        let mut engine = None;
        self.shared.change(|s| {
            s.stop = true;
            engine = s.engine.take();
        });
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        vidcut_av::debug!(
            "viewer",
            "stopped in {:.0} ms",
            started.elapsed().as_secs_f64() * 1000.0
        );
        if let Some(engine) = engine {
            let _ = crate::threads::spawn(
                "vidcut-viewer-close",
                crate::threads::Work::Interactive,
                move || {
                    let started = Instant::now();
                    drop(engine);
                    vidcut_av::debug!(
                        "viewer",
                        "the player ended in {:.0} ms",
                        started.elapsed().as_secs_f64() * 1000.0
                    );
                },
            );
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
        // A picture copied down or uploaded counts for the engine shown.
        let counting = engine
            .as_ref()
            .map(|e| vidcut_av::copies::Guard::set(e.copies()));
        let drawn = screen.draw(
            gpu,
            shown.as_ref().map(|s| &s.picture),
            w.max(2) as u32,
            h.max(2) as u32,
        );
        drop(counting);
        let changed = drawn != Drawn::Same;
        if changed && vidcut_av::debug() {
            eprintln!("[viewer] drew {} at {w}x{h}", drew(shown.as_ref(), &screen));
        }
        if drawn == Drawn::Target {
            generation += 1;
        }
        let target = screen.view().map(|view| Target {
            generation,
            view: view.clone(),
        });
        let has_frame = shown.is_some();
        let shown_how = shown.as_ref().map(|s| how(s, &screen));
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
            s.stats.size = [w.max(2) as u32, h.max(2) as u32];
            s.stats.shown = shown_how.map(String::from);
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

/// How the picture shown got on screen.
fn how(shown: &vidcut_av::player::Shown, screen: &Screen) -> &'static str {
    match shown.picture.gpu() {
        None => "uploaded",
        Some(_) if screen.shared() => "shown from GPU memory",
        Some(_) => "copied down and uploaded",
    }
}

/// wgpu's backend as people name it: `Vulkan`, `Direct3D 12`.
fn backend_name(backend: wgpu::Backend) -> String {
    match backend {
        wgpu::Backend::Vulkan => "Vulkan".into(),
        wgpu::Backend::Metal => "Metal".into(),
        wgpu::Backend::Dx12 => "Direct3D 12".into(),
        wgpu::Backend::Gl => "OpenGL".into(),
        other => format!("{other:?}"),
    }
}

/// What the viewer drew, for `VIDCUT_DEBUG`: the frame, the picture's size,
/// layout and colors, where it was, what decoded it and how it got on
/// screen.
fn drew(shown: Option<&vidcut_av::player::Shown>, screen: &Screen) -> String {
    let Some(s) = shown else {
        return "nothing".into();
    };
    let p = &s.picture;
    let how = how(s, screen);
    let c = p.colors();
    let hdr = c.hdr.map_or(String::new(), |h| format!(", {h:?}"));
    let range = if c.full_range { "full" } else { "video" };
    let turned = match p.rotation() {
        0 => String::new(),
        r => format!(", turned {r}°"),
    };
    let late = if s.exact {
        ""
    } else {
        " (still the picture before it)"
    };
    format!(
        "frame {}{late}: {}x{} {:?}, {:?} {range} range{hdr}{turned}, in {} memory, \
         decoded by {}, {how}",
        s.frame,
        p.width(),
        p.height(),
        p.layout(),
        c.matrix,
        p.gpu().map_or("CPU", |k| k.label()),
        p.decoder().map_or("nothing (black)", |d| d.name()),
    )
}
