//! The two-up's cells (the Trim Editor's and Dynamic Trim's two frames, of
//! a copy of the project): a player each (docs/decisions/preview.md §9),
//! without sound, on the media of the clip the cell shows, told the source
//! frame as the trim moves. The viewer's shader draws their pictures
//! ([`Screens`]).

use std::sync::Arc;
use std::time::Instant;

use eframe::egui;
use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use vidcut_av::hw::Kind;
use vidcut_av::player::{Config, ManualOutput, Picture, Player, Shown};
use vidcut_core::model::Project;
use vidcut_core::render::{RenderPlan, VideoPiece};

use super::picture::{Drawn, Gpu, Screen};

/// Frames the cells were asked for since the GUI started, and those they
/// never showed: another was asked for before the frame's picture was there.
#[derive(Debug, Clone, Copy, Default)]
pub struct Counts {
    pub asked: u64,
    pub passed_over: u64,
}

struct Cell {
    player: Player,
    plan: RenderPlan,
    /// The frame asked for last, whether it showed, and the speed the
    /// player follows it at.
    asked: i64,
    shown: bool,
    speed: f64,
}

pub struct Cells {
    ctx: egui::Context,
    /// The decoders whose pictures the screens show from GPU memory.
    gpu_pictures: Vec<Kind>,
    cells: [Option<Cell>; 2],
    counts: Counts,
}

impl Cells {
    pub fn new(ctx: egui::Context, gpu_pictures: Vec<Kind>) -> Self {
        Self {
            ctx,
            gpu_pictures,
            cells: [None, None],
            counts: Counts::default(),
        }
    }

    pub fn counts(&self) -> Counts {
        self.counts
    }

    /// The two-up closed: the players, their decoders and pictures go.
    pub fn close(&mut self) {
        self.cells = [None, None];
    }

    /// Cell `i` shows `piece`, a frame of a clip of `p` alone; `speed` is
    /// the trim's run (× real time, 0 standing still). What it shows now.
    pub fn show(&mut self, i: usize, p: &Project, piece: &VideoPiece, speed: f64) -> Option<Shown> {
        let (plan, frame) = plan(p, piece)?;
        let format = |plan: &RenderPlan| (plan.sample_rate, plan.channels);
        if self.cells[i]
            .as_ref()
            .is_none_or(|c| format(&c.plan) != format(&plan))
        {
            // A quarter of the viewer's memory each, half of it together.
            let ctx = self.ctx.clone();
            let config = Config {
                budget: crate::preferences::player_budget().part(4),
                audio_scrubbing: false,
                on_picture: Some(Arc::new(move || ctx.request_repaint())),
                hardware: crate::preferences::gpu_decoding().on,
                gpu_pictures: self.gpu_pictures.clone(),
            };
            let player = Player::new(plan.clone(), config, Box::new(ManualOutput::new())).ok()?;
            player.seek_quietly(frame);
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
        // Running, the way its frame moves.
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
        let shown = c.player.present(Instant::now());
        c.shown |= shown.as_ref().is_some_and(|s| s.exact);
        shown
    }
}

/// The plan a cell plays for `piece` and its frame there: the clip's media
/// whole, so a run moves along one plan; black for no picture.
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
                range: (0, 1),
                name: "black".into(),
            },
            0,
        ),
    };
    plan.audio.clear();
    Some((plan, frame))
}

/// Draws the cells' pictures with the viewer's shader, each into a target
/// of its cell's size, which egui shows as a native texture.
pub struct Screens {
    render: RenderState,
    gpu: Gpu,
    screens: [Screen; 2],
    textures: [Option<egui::TextureId>; 2],
}

impl Screens {
    pub fn new(render: &RenderState) -> Self {
        Self {
            render: render.clone(),
            gpu: Gpu::new(&render.device, &render.queue),
            screens: [Screen::new(), Screen::new()],
            textures: [None, None],
        }
    }

    pub fn gpu_pictures(&self) -> Vec<Kind> {
        self.gpu.gpu_pictures()
    }

    /// Cell `i`'s texture, `picture` drawn into it at `size` (physical
    /// pixels).
    pub fn draw(
        &mut self,
        i: usize,
        picture: &Arc<Picture>,
        size: [u32; 2],
    ) -> Option<egui::TextureId> {
        let drawn = self.screens[i].draw(&self.gpu, Some(picture), size[0], size[1]);
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
