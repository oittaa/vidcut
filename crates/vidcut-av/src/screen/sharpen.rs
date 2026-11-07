use vidcut_core::place::Sharpening;

use super::Gpu;
use crate::compute::{buffer, buffer_with, group};

const LABEL: &str = "vidcut sharpen";

const STORED: wgpu::BufferUsages = wgpu::BufferUsages::STORAGE;

const WIDE: u32 = 16;

const LIGHT: [f64; 3] = [0.2126, 0.7152, 0.0722];

const RED: f64 = 0.65;

const BLUE: f64 = 0.552;

const SOLVES: [(f64, u32); 3] = [(1.0, 15), (4.0, 15), (32.0, 20)];

const FIRST: usize = 1;

pub(super) struct Pipes {
    split: wgpu::ComputePipeline,
    linked: wgpu::ComputePipeline,
    sweep: wgpu::ComputePipeline,
    collect: wgpu::ComputePipeline,
}

impl Pipes {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(LABEL),
            source: wgpu::ShaderSource::Wgsl(include_str!("sharpen.wgsl").into()),
        });
        let made = |entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(LABEL),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Self {
            split: made("split"),
            linked: made("linked"),
            sweep: made("sweep"),
            collect: made("collect"),
        }
    }
}

pub(super) struct Planes {
    each: usize,
    count: usize,
    guide: wgpu::Buffer,
    across: wgpu::Buffer,
    down: wgpu::Buffer,
    solved: wgpu::Buffer,
    made: wgpu::Buffer,
}

impl Planes {
    fn new(device: &wgpu::Device, each: usize, count: usize) -> Self {
        let one = || buffer(device, LABEL, each, STORED);
        Self {
            each,
            count,
            guide: one(),
            across: one(),
            down: one(),
            solved: one(),
            made: buffer(device, LABEL, each * count, STORED),
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Call {
    at: u32,
    parity: u32,
    weights: [f64; 3],
    strength: f64,
    keep: bool,
    gain: f64,
}

fn call_bytes(c: &Call, (w, h): (u32, u32)) -> Vec<u8> {
    let mut out = Vec::with_capacity(48);
    for v in [w, h, c.at, c.parity] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let keep = if c.keep { 1.0 } else { 0.0 };
    let [r, g, b] = c.weights;
    for v in [r, g, b, c.strength, keep, c.gain, 0.0, 0.0] {
        out.extend_from_slice(&(v as f32).to_le_bytes());
    }
    out
}

struct Plane {
    weights: [f64; 3],
    unit: f64,
    gains: [f64; 3],
}

fn plane_list(s: &Sharpening) -> [Plane; 3] {
    let [kr, kg, kb] = LIGHT;
    let scaled = |scale: f64, v: [f64; 3]| v.map(|c| c * scale);
    [
        Plane {
            weights: LIGHT,
            unit: 1.0,
            gains: s.light,
        },
        Plane {
            weights: scaled(RED, [1.0 - kr, -kg, -kb]),
            unit: 1.0 / RED,
            gains: s.color,
        },
        Plane {
            weights: scaled(BLUE, [-kr, -kg, 1.0 - kb]),
            unit: 1.0 / BLUE,
            gains: s.color,
        },
    ]
}

struct Solving<'a> {
    gpu: &'a Gpu,
    pipes: &'a Pipes,
    planes: &'a Planes,
    size: (u32, u32),
    taken: &'a wgpu::TextureView,
    fine: f64,
}

impl Solving<'_> {
    fn params(&self, c: &Call) -> wgpu::Buffer {
        buffer_with(
            &self.gpu.device,
            LABEL,
            &call_bytes(c, self.size),
            wgpu::BufferUsages::UNIFORM,
        )
    }

    fn all(&self, pass: &mut wgpu::ComputePass<'_>) {
        let (w, h) = self.size;
        pass.dispatch_workgroups(w.div_ceil(WIDE), h.div_ceil(WIDE), 1);
    }

    fn split(&self, pass: &mut wgpu::ComputePass<'_>, base: &Call) {
        let params = self.params(base);
        let bind = self
            .gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(LABEL),
                layout: &self.pipes.split.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(self.taken),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.planes.guide.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.planes.solved.as_entire_binding(),
                    },
                ],
            });
        pass.set_pipeline(&self.pipes.split);
        pass.set_bind_group(0, &bind, &[]);
        self.all(pass);
    }

    fn collect(&self, pass: &mut wgpu::ComputePass<'_>, c: &Call) {
        let p = self.planes;
        let params = self.params(c);
        let bind = group(
            &self.gpu.device,
            &self.pipes.collect,
            &[&params, &p.guide, &p.solved, &p.made],
        );
        pass.set_pipeline(&self.pipes.collect);
        pass.set_bind_group(0, &bind, &[]);
        self.all(pass);
    }

    fn links(&self, pass: &mut wgpu::ComputePass<'_>, base: &Call) {
        let p = self.planes;
        let params = self.params(base);
        let bind = group(
            &self.gpu.device,
            &self.pipes.linked,
            &[&params, &p.guide, &p.across, &p.down],
        );
        pass.set_pipeline(&self.pipes.linked);
        pass.set_bind_group(0, &bind, &[]);
        self.all(pass);
    }

    fn solve(
        &self,
        pass: &mut wgpu::ComputePass<'_>,
        base: &Call,
        (wide, half_sweeps): (f64, u32),
    ) {
        let p = self.planes;
        let strength = wide * self.fine * self.fine;
        let binds = [0, 1].map(|parity| {
            let params = self.params(&Call {
                parity,
                strength,
                ..*base
            });
            group(
                &self.gpu.device,
                &self.pipes.sweep,
                &[&params, &p.guide, &p.across, &p.down, &p.solved],
            )
        });
        pass.set_pipeline(&self.pipes.sweep);
        let mut parity = FIRST;
        for _ in 0..half_sweeps {
            pass.set_bind_group(0, &binds[parity], &[]);
            self.all(pass);
            parity = 1 - parity;
        }
    }

    fn plane(&self, pass: &mut wgpu::ComputePass<'_>, n: usize, plane: &Plane) {
        let (w, h) = self.size;
        let base = Call {
            at: n as u32 * w * h,
            weights: plane.weights,
            ..Call::default()
        };
        let [k1, k2, k3] = plane.gains;
        self.split(pass, &base);
        if plane.gains == [0.0; 3] {
            self.collect(pass, &base);
            return;
        }
        self.links(pass, &base);
        let gains = [k2 - k1, k3 - k2, -k3];
        for (j, (one, gain)) in SOLVES.into_iter().zip(gains).enumerate() {
            self.solve(pass, &base, one);
            self.collect(
                pass,
                &Call {
                    keep: j > 0,
                    gain: gain * plane.unit,
                    ..base
                },
            );
        }
    }
}

pub(super) fn planes<'a>(
    gpu: &Gpu,
    s: &Sharpening,
    (taken, size): (&wgpu::TextureView, wgpu::Extent3d),
    kept: &'a mut Option<Planes>,
    encoder: &mut wgpu::CommandEncoder,
) -> &'a wgpu::Buffer {
    let device = &gpu.device;
    let (w, h) = (size.width, size.height);
    let each = w as usize * h as usize * 4;
    let count = if s.colors() { 3 } else { 1 };
    if kept
        .as_ref()
        .is_none_or(|p| p.each != each || p.count < count)
    {
        *kept = Some(Planes::new(device, each, count));
    }
    let planes = kept.as_ref().expect("made above");
    let solving = Solving {
        gpu,
        pipes: gpu.sharpen.get_or_init(|| Pipes::new(device)),
        planes,
        size: (w, h),
        taken,
        fine: s.size,
    };
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some(LABEL),
        timestamp_writes: None,
    });
    for (n, plane) in plane_list(s).iter().enumerate().take(count) {
        solving.plane(&mut pass, n, plane);
    }
    drop(pass);
    &planes.made
}
