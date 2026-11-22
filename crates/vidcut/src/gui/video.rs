use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use eframe::egui;
use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use eframe::wgpu::FilterMode::{Linear, Nearest};
use vidcut_av::player::Player;
use vidcut_core::Error;

use super::workspace::{NodeShots, ScopePictures};
use crate::live::stats::{PlayerStats, ViewerStats};
use crate::live::{Capture, Presented};
use vidcut_av::screen::Gate;
use vidcut_av::screen::{Drawn, Gpu, Lut, Screen, ShotsShown, ShotsWanted};
use vidcut_core::model::Hdr;
use vidcut_core::preferences::{Scope, ScopeScale};

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
    upscale: bool,
    scopes: Vec<(Scope, [i32; 2])>,
    scale: ScopeScale,
    target: Option<Target>,
    scope_targets: Vec<(Scope, Target)>,
    shots: Option<ShotsWanted>,
    shots_shown: Option<ShotsShown>,
    display_lut: Option<PathBuf>,
    display_why: Option<String>,
    colors: Option<Hdr>,
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
    gpu_pictures: vidcut_av::hw::GpuPictures,
    thread: Option<JoinHandle<()>>,
    texture: Option<(u64, egui::TextureId)>,
    scope_textures: Vec<(Scope, (u64, egui::TextureId))>,
    shots_texture: Option<(u64, egui::TextureId)>,
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
        vidcut_av::player::set_viewer_gpu(vidcut_av::gpus::adapter_id(
            info.vendor,
            info.device,
            &info.device_pci_bus_id,
        ));
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
            scope_textures: Vec::new(),
            shots_texture: None,
        }
    }

    pub fn gpu_pictures(&self) -> vidcut_av::hw::GpuPictures {
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
        upscale: bool,
    ) -> (Option<egui::TextureId>, bool) {
        let (target, has_frame) = {
            let mut s = self.shared.lock();
            let changed = s.want != want || s.upscale != upscale;
            s.want = want;
            s.upscale = upscale;
            if changed {
                s.woken = true;
                self.shared.wake.notify_all();
            }
            (s.target.clone(), s.has_frame)
        };
        if let Some(t) = &target {
            self.texture = Some(native_texture(render, t, self.texture, Nearest));
        }
        (self.texture.map(|(_, t)| t), has_frame)
    }

    pub fn show_lut(&self, path: Option<&Path>) {
        let mut s = self.shared.lock();
        if s.display_lut.as_deref() != path {
            s.display_lut = path.map(Path::to_path_buf);
            s.woken = true;
            self.shared.wake.notify_all();
        }
    }

    pub fn lut_failed(&self) -> Option<String> {
        self.shared.lock().display_why.take()
    }

    pub fn sync_scopes(
        &mut self,
        render: &RenderState,
        wanted: &[(Scope, [i32; 2])],
        scale: ScopeScale,
    ) -> ScopePictures {
        let (targets, colors) = {
            let mut s = self.shared.lock();
            if s.scopes != wanted || s.scale != scale {
                s.scopes = wanted.to_vec();
                s.scale = scale;
                s.woken = true;
                self.shared.wake.notify_all();
            }
            (s.scope_targets.clone(), s.colors)
        };
        self.scope_textures.retain(|(scope, (_, tex))| {
            let kept = targets.iter().any(|(s, _)| s == scope);
            if !kept {
                render.renderer.write().free_texture(tex);
            }
            kept
        });
        let mut textures = Vec::with_capacity(targets.len());
        for (scope, target) in &targets {
            let held = self
                .scope_textures
                .iter_mut()
                .find(|(s, _)| s == scope)
                .map(|(_, h)| h);
            let made = match held {
                Some(h) => {
                    *h = native_texture(render, target, Some(*h), Nearest);
                    h.1
                }
                None => {
                    let h = native_texture(render, target, None, Nearest);
                    self.scope_textures.push((*scope, h));
                    h.1
                }
            };
            textures.push((*scope, made));
        }
        ScopePictures { textures, colors }
    }

    pub fn sync_shots(&mut self, render: &RenderState, wanted: Option<&ShotsWanted>) -> NodeShots {
        let shown = {
            let mut s = self.shared.lock();
            if s.shots.as_ref() != wanted {
                s.shots = wanted.cloned();
                s.woken = true;
                self.shared.wake.notify_all();
            }
            s.shots_shown.clone()
        };
        let Some(shown) = shown else {
            if let Some((_, tex)) = self.shots_texture.take() {
                render.renderer.write().free_texture(&tex);
            }
            return NodeShots::default();
        };
        let target = Target {
            generation: shown.generation,
            view: shown.view,
        };
        let held = native_texture(render, &target, self.shots_texture, Linear);
        self.shots_texture = Some(held);
        let (w, h) = (shown.size.0 as f32, shown.size.1 as f32);
        let cells = (shown.cells.into_iter())
            .map(|(node, [x, y, cw, ch])| {
                let [x, y, cw, ch] = [x, y, cw, ch].map(|v| v as f32);
                let uv = egui::Rect::from_min_max(
                    egui::pos2(x / w, y / h),
                    egui::pos2((x + cw) / w, (y + ch) / h),
                );
                (node, uv, egui::vec2(cw, ch))
            })
            .collect();
        NodeShots {
            texture: Some(held.1),
            cells,
        }
    }
}

fn native_texture(
    render: &RenderState,
    target: &Target,
    held: Option<(u64, egui::TextureId)>,
    filter: wgpu::FilterMode,
) -> (u64, egui::TextureId) {
    match held {
        Some((g, tex)) if g == target.generation => (g, tex),
        Some((_, tex)) => {
            render
                .renderer
                .write()
                .update_egui_texture_from_wgpu_texture(&render.device, &target.view, filter, tex);
            (target.generation, tex)
        }
        None => {
            let tex = render.renderer.write().register_native_texture(
                &render.device,
                &target.view,
                filter,
            );
            (target.generation, tex)
        }
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

#[derive(Default)]
struct DisplayLut {
    asked: Option<PathBuf>,
    kept: Option<(String, Lut)>,
    shown: Option<Lut>,
}

impl DisplayLut {
    fn ask(&mut self, path: Option<PathBuf>) -> Option<String> {
        if self.asked == path {
            return None;
        }
        self.asked = path;
        self.shown = None;
        let path = self.asked.as_deref()?;
        let stamp = vidcut_core::cube::stamp(path);
        if let Some((kept, lut)) = &self.kept
            && *kept == stamp
        {
            self.shown = Some(lut.clone());
            return None;
        }
        match vidcut_av::screen::load_lut(path) {
            Ok(lut) => {
                self.kept = Some((stamp, lut.clone()));
                self.shown = Some(lut);
                None
            }
            Err(e) => Some(e.to_string()),
        }
    }
}

fn run(shared: &Shared, gpu: &Gpu, repaint: &dyn Fn()) {
    let mut screen = Screen::new();
    let mut display = DisplayLut::default();
    let mut generation = 0u64;
    let mut next: Option<Instant> = None;
    loop {
        let (tasks, engine, want, upscale, scopes, scale, shots, lut) = {
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
            (
                std::mem::take(&mut s.tasks),
                s.engine.clone(),
                s.want,
                s.upscale,
                s.scopes.clone(),
                s.scale,
                s.shots.clone(),
                s.display_lut.clone(),
            )
        };
        if let Some(why) = display.ask(lut) {
            shared.lock().display_why = Some(why);
        }
        let now = Instant::now();
        let shown = engine.as_ref().and_then(|e| e.present(now));
        let [w, h] = want;
        let counting = engine
            .as_ref()
            .map(|e| vidcut_av::copies::Guard::set(e.copies()));
        let kernels: Vec<u32> = shown
            .as_ref()
            .filter(|_| upscale)
            .map(|s| {
                s.scales
                    .iter()
                    .map(|&k| vidcut_av::screen::kernel(k))
                    .collect()
            })
            .unwrap_or_default();
        screen.set_output_lut(shown.as_ref().and_then(|s| s.output_lut.clone()));
        screen.set_display_lut(display.shown.clone());
        screen.set_timeline_grade(shown.as_ref().and_then(|s| s.timeline_grade.as_ref()));
        screen.set_frame_size(shown.as_ref().map(|s| s.size));
        screen.scoping(!scopes.is_empty());
        screen.set_shots(shots);
        let drawn = screen.draw(
            gpu,
            shown.as_ref().map(|s| &s.picture),
            shown.as_ref().map_or(&[][..], |s| &s.titles),
            (
                shown.as_ref().map_or(&[][..], |s| &s.placed),
                &kernels,
                shown.as_ref().and_then(|s| s.colors),
            ),
            w.max(2) as u32,
            h.max(2) as u32,
        );
        drop(counting);
        let wanted: Vec<(Scope, (u32, u32))> = scopes
            .iter()
            .map(|&(scope, [w, h])| (scope, (w.max(2) as u32, h.max(2) as u32)))
            .collect();
        let plotted = screen.scopes(gpu, &wanted, scale);
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
        let scope_targets: Vec<(Scope, Target)> = screen
            .scope_views()
            .into_iter()
            .map(|(scope, generation, view)| (scope, Target { generation, view }))
            .collect();
        let shots_shown = screen.shots();
        let colors = shown.as_ref().and_then(|s| s.colors);
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
            s.scope_targets = scope_targets;
            s.shots_shown = shots_shown;
            s.colors = colors;
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
        if changed || plotted {
            repaint();
        }
    }
}

fn how(shown: &vidcut_av::player::Shown, screen: &Screen) -> &'static str {
    let p = &shown.picture;
    match (p.gpu(), p.held()) {
        (None, false) => "uploaded",
        (Some(_), _) if screen.shared() => "shown from GPU memory",
        (None, true) if screen.held_shared() => "shown from GPU memory",
        _ => "copied down and uploaded",
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
