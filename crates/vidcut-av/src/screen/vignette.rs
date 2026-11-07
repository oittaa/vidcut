use vidcut_core::place::Shading;

use super::Gpu;
use crate::compute::{buffer, buffer_with, group};

const CELLS: wgpu::BufferUsages = wgpu::BufferUsages::STORAGE;

const WIDE: u32 = 16;

pub(super) struct Pipes {
    mask: wgpu::ComputePipeline,
    run: wgpu::ComputePipeline,
}

impl Pipes {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vidcut vignette"),
            source: wgpu::ShaderSource::Wgsl(include_str!("vignette.wgsl").into()),
        });
        let made = |entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("vidcut vignette"),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Self {
            mask: made("mask"),
            run: made("run"),
        }
    }
}

pub(super) struct Cells {
    bytes: usize,
    kept: [wgpu::Buffer; 2],
}

fn params_bytes(s: &Shading, width: u32, down: bool) -> Vec<u8> {
    let (sin, cos) = s.turn.to_radians().sin_cos();
    let mut out = Vec::with_capacity(48);
    for v in [s.cells[0], s.cells[1], width, u32::from(down)] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for v in [s.radius, s.stretch, cos, sin] {
        out.extend_from_slice(&(v as f32).to_le_bytes());
    }
    for v in [s.shape.code(), 0, 0, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

pub(super) fn cells<'a>(
    gpu: &Gpu,
    s: &Shading,
    kept: &'a mut Option<Cells>,
    encoder: &mut wgpu::CommandEncoder,
) -> &'a wgpu::Buffer {
    let device = &gpu.device;
    let [w, h] = s.cells;
    let bytes = w as usize * h as usize * 4;
    if kept.as_ref().is_none_or(|c| c.bytes < bytes) {
        *kept = Some(Cells {
            bytes,
            kept: std::array::from_fn(|_| buffer(device, "vidcut vignette", bytes, CELLS)),
        });
    }
    let made = kept.as_ref().expect("made above");
    let pipes = gpu.vignette.get_or_init(|| Pipes::new(device));
    let params = |width, down| {
        buffer_with(
            device,
            "vidcut vignette",
            &params_bytes(s, width, down),
            wgpu::BufferUsages::UNIFORM,
        )
    };
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("vidcut vignette"),
        timestamp_writes: None,
    });
    pass.set_pipeline(&pipes.mask);
    let first = params(0, false);
    pass.set_bind_group(
        0,
        &group(device, &pipes.mask, &[&first, &made.kept[0]]),
        &[],
    );
    pass.dispatch_workgroups(w.div_ceil(WIDE), h.div_ceil(WIDE), 1);
    pass.set_pipeline(&pipes.run);
    let across = s.boxes.map(|width| (width, false));
    let down = s.boxes.map(|width| (width, true));
    for (j, (width, down)) in across.into_iter().chain(down).enumerate() {
        let lines = if down { w } else { h };
        let step = params(width, down);
        let (from, to) = (&made.kept[j % 2], &made.kept[1 - j % 2]);
        pass.set_bind_group(0, &group(device, &pipes.run, &[&step, from, to]), &[]);
        pass.dispatch_workgroups(lines.div_ceil(WIDE), 1, 1);
    }
    drop(pass);
    &made.kept[0]
}
