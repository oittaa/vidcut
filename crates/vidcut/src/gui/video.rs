use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use eframe::egui;
use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use vidcut_av::player::Player;
use vidcut_core::Error;

use super::gate::Gate;
use super::picture::{Drawn, Gpu, Screen};
use crate::live::stats::{PlayerStats, ViewerStats};
use crate::live::{Capture, Presented};

type Engine = Arc<Player>;

enum Task {
    Present(Presented),
    Capture(Capture),
}

#[derive(Clone)]
struct Target {
    generation: u64,
    view: wgpu::TextureView,
}

#[derive(Default)]
struct State {
    tasks: Vec<Task>,
    engine: Option<Engine>,
    want: [i32; 2],
    target: Option<Target>,
    has_frame: bool,
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

pub struct Viewer {
    shared: Arc<Shared>,
    gpu_pictures: Vec<vidcut_av::hw::Kind>,
    thread: Option<JoinHandle<()>>,
    texture: Option<(u64, egui::TextureId)>,
}

#[derive(Clone)]
pub struct ViewerHandle {
    shared: Arc<Shared>,
}

const IDLE: Duration = Duration::from_millis(100);

impl Viewer {
    pub fn start(
        render: &RenderState,
        gate: Arc<Gate>,
        repaint: impl Fn() + Send + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                want: [640, 360],
                ..State::default()
            }),
            wake: Condvar::new(),
        });
        let gpu = Gpu::new(&render.device, &render.queue).behind(gate);
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

    pub fn gpu_pictures(&self) -> Vec<vidcut_av::hw::Kind> {
        self.gpu_pictures.clone()
    }

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
    pub fn attach(&self, engine: Option<Engine>) {
        self.shared.change(|s| s.engine = engine);
    }

    pub fn present(&self, done: Presented) {
        self.shared.change(|s| s.tasks.push(Task::Present(done)));
    }

    pub fn capture(&self, done: Capture) {
        self.shared.change(|s| s.tasks.push(Task::Capture(done)));
    }

    pub fn wake(&self) {
        self.shared.change(|_| {});
    }

    pub fn stats(&self) -> ViewerStats {
        self.shared.lock().stats.clone()
    }
}

impl Viewer {
    pub fn levels(&self) -> Option<vidcut_av::player::Levels> {
        let engine = self.shared.lock().engine.clone()?;
        engine.levels(Instant::now())
    }
}

impl Viewer {
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
            let _ = crate::threads::closing(
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

fn run(shared: &Shared, gpu: &Gpu, repaint: &dyn Fn()) {
    let mut screen = Screen::new();
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
        let counting = engine
            .as_ref()
            .map(|e| vidcut_av::copies::Guard::set(e.copies()));
        let drawn = screen.draw(
            gpu,
            shown.as_ref().map(|s| &s.picture),
            shown.as_ref().map_or(&[][..], |s| &s.titles),
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

fn how(shown: &vidcut_av::player::Shown, screen: &Screen) -> &'static str {
    match shown.picture.gpu() {
        None => "uploaded",
        Some(_) if screen.shared() => "shown from GPU memory",
        Some(_) => "copied down and uploaded",
    }
}

fn backend_name(backend: wgpu::Backend) -> String {
    match backend {
        wgpu::Backend::Vulkan => "Vulkan".into(),
        wgpu::Backend::Metal => "Metal".into(),
        wgpu::Backend::Dx12 => "Direct3D 12".into(),
        wgpu::Backend::Gl => "OpenGL".into(),
        other => format!("{other:?}"),
    }
}

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
        "frame {}{late}: {}x{} {:?}/{:?}/{range}{hdr}{turned}, {} memory, by {}, {how}",
        s.frame,
        p.width(),
        p.height(),
        p.layout(),
        c.matrix,
        p.gpu().map_or("CPU", |k| k.label()),
        p.decoder().map_or("nothing (black)", |d| d.name()),
    )
}
