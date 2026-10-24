use std::sync::Arc;
use std::time::Instant;

use eframe::egui;
use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use vidcut_av::hw::Kind;
use vidcut_av::player::{Budget, Config, ManualOutput, Picture, Player, Shown};
use vidcut_core::model::Project;
use vidcut_core::render::{RenderPlan, VideoPiece};

use super::picture::{Drawn, Gpu, Screen};

#[derive(Debug, Clone, Copy, Default)]
pub struct Counts {
    pub asked: u64,
    pub passed_over: u64,
}

struct Cell {
    player: Player,
    plan: RenderPlan,
    asked: i64,
    shown: bool,
    speed: f64,
}

pub struct Cells {
    ctx: egui::Context,
    gpu_pictures: Vec<Kind>,
    budget: Budget,
    cells: [Option<Cell>; CELLS],
    counts: Counts,
}

pub const CELLS: usize = 4;

impl Cells {
    pub fn new(ctx: egui::Context, gpu_pictures: Vec<Kind>, viewer: Budget) -> Self {
        Self {
            ctx,
            gpu_pictures,
            budget: viewer.part(CELLS),
            cells: std::array::from_fn(|_| None),
            counts: Counts::default(),
        }
    }

    pub fn counts(&self) -> Counts {
        self.counts
    }

    pub fn close(&mut self) {
        self.keep(0);
    }

    pub fn keep(&mut self, n: usize) {
        let cells: Vec<Cell> = self
            .cells
            .iter_mut()
            .skip(n)
            .filter_map(Option::take)
            .collect();
        if cells.is_empty() {
            return;
        }
        let _ = crate::threads::closing(
            "vidcut-two-up-close",
            crate::threads::Work::Interactive,
            move || {
                let started = Instant::now();
                drop(cells);
                vidcut_av::debug!(
                    "gui",
                    "the two-up's players ended in {:.0} ms",
                    started.elapsed().as_secs_f64() * 1000.0
                );
            },
        );
    }

    pub fn show(
        &mut self,
        i: usize,
        p: &Project,
        piece: &VideoPiece,
        size: [u32; 2],
        speed: f64,
    ) -> Option<Shown> {
        let (plan, frame) = plan(p, piece)?;
        let format = |plan: &RenderPlan| (plan.sample_rate, plan.channels);
        if self.cells[i]
            .as_ref()
            .is_none_or(|c| format(&c.plan) != format(&plan))
        {
            let ctx = self.ctx.clone();
            let config = Config {
                budget: self.budget,
                audio_scrubbing: false,
                on_picture: Some(Arc::new(move || ctx.request_repaint())),
                hardware: crate::preferences::gpu_decoding().on,
                gpu_pictures: self.gpu_pictures.clone(),
            };
            let player = Player::new(plan.clone(), config, Box::new(ManualOutput::new())).ok()?;
            player.follow(frame, 0.0);
            self.counts.asked += 1;
            self.cells[i] = Some(Cell {
                player,
                plan: plan.clone(),
                asked: frame,
                shown: false,
                speed: 0.0,
            });
        }
        let c = self.cells[i].as_mut()?;
        if c.plan != plan {
            c.player.set_plan_at(plan.clone(), frame).ok()?;
            c.plan = plan;
            c.speed = 0.0;
        }
        let speed = match frame - c.asked {
            _ if speed == 0.0 => 0.0,
            0 => c.speed,
            d => speed.abs() * d.signum() as f64,
        };
        if frame != c.asked {
            self.counts.asked += 1;
            if !c.shown {
                self.counts.passed_over += 1;
            }
            c.shown = false;
        }
        if frame != c.asked || speed != c.speed {
            c.player.follow(frame, speed);
            c.asked = frame;
            c.speed = speed;
        }
        c.player.shrink_to(Some((size[0], size[1])));
        let shown = c.player.present(Instant::now());
        c.shown |= shown.as_ref().is_some_and(|s| s.exact);
        shown
    }
}

impl Drop for Cells {
    fn drop(&mut self) {
        self.close();
    }
}

fn plan(p: &Project, piece: &VideoPiece) -> Option<(RenderPlan, i64)> {
    let s = &p.settings;
    let (mut plan, frame) = match piece {
        VideoPiece::Source { media, .. } => (
            vidcut_core::render::tape(p, std::slice::from_ref(media))
                .ok()?
                .0,
            piece.position(0),
        ),
        _ => (
            RenderPlan {
                fps: s.fps,
                width: s.width,
                height: s.height,
                sample_rate: s.sample_rate,
                channels: s.channels,
                frames: 1,
                video: vec![VideoPiece::Black { frames: 1 }],
                audio: Vec::new(),
                ducking: Vec::new(),
                range: (0, 1),
                name: "black".into(),
            },
            0,
        ),
    };
    plan.audio.clear();
    Some((plan, frame))
}

pub struct Screens {
    render: RenderState,
    gpu: Gpu,
    screens: [Screen; CELLS],
    textures: [Option<egui::TextureId>; CELLS],
}

impl Screens {
    pub fn new(render: &RenderState) -> Self {
        Self {
            render: render.clone(),
            gpu: Gpu::new(&render.device, &render.queue),
            screens: std::array::from_fn(|_| Screen::new()),
            textures: [None; CELLS],
        }
    }

    pub fn gpu_pictures(&self) -> Vec<Kind> {
        self.gpu.gpu_pictures()
    }

    pub fn draw(
        &mut self,
        i: usize,
        picture: &Arc<Picture>,
        size: [u32; 2],
    ) -> Option<egui::TextureId> {
        let drawn = self.screens[i].draw(
            &self.gpu,
            Some(picture),
            &[],
            (&[], &[], &[]),
            size[0],
            size[1],
        );
        let view = self.screens[i].view()?;
        let filter = wgpu::FilterMode::Nearest;
        let mut renderer = self.render.renderer.write();
        match self.textures[i] {
            Some(t) if drawn == Drawn::Target => {
                renderer.update_egui_texture_from_wgpu_texture(&self.render.device, view, filter, t)
            }
            Some(_) => {}
            None => {
                self.textures[i] =
                    Some(renderer.register_native_texture(&self.render.device, view, filter))
            }
        }
        self.textures[i]
    }
}
