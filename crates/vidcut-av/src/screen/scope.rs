use vidcut_core::generators::{hlg_code, pq_code};
use vidcut_core::model::Hdr;
use vidcut_core::preferences::{Scope, ScopeScale};

use super::{Gpu, TARGET, Target, begin, make_target};
use crate::color::{Colors, Matrix, shader_colors};

pub const SMALLEST: u32 = 16;

pub const LARGEST: u32 = 4096;

const LOW: f64 = -0.1;

const HIGH: f64 = 1.1;

const REACH: f64 = 0.55;

const CIRCLE: f64 = 0.5;

const BARS: f64 = 0.75;

const FULL_AT: u64 = 16;

const PARAMS: usize = 176;

const MOST_ROWS: usize = 12;

pub(super) struct Pipes {
    waveform: wgpu::ComputePipeline,
    parade: wgpu::ComputePipeline,
    vectorscope: wgpu::ComputePipeline,
    painting: wgpu::RenderPipeline,
}

impl Pipes {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vidcut scope"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scope.wgsl").into()),
        });
        let gather = |entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("vidcut scope"),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let painting = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vidcut scope"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TARGET,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            waveform: gather("waveform"),
            parade: gather("parade"),
            vectorscope: gather("vectorscope"),
            painting,
        }
    }

    fn gathering(&self, scope: Scope) -> &wgpu::ComputePipeline {
        match scope {
            Scope::Waveform => &self.waveform,
            Scope::Parade => &self.parade,
            Scope::Vectorscope => &self.vectorscope,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub label: String,
    pub at: [f32; 2],
}

fn nits_marks(nits: &[f64], code: fn(f64) -> f64) -> Vec<(String, f64)> {
    nits.iter().map(|&n| (n.to_string(), code(n))).collect()
}

fn steps(count: u32, label: impl Fn(u32) -> u32) -> Vec<(String, f64)> {
    (0..=count)
        .map(|i| (label(i).to_string(), f64::from(i) / f64::from(count)))
        .collect()
}

fn levels(scale: ScopeScale, colors: Option<Hdr>) -> Vec<(String, f64)> {
    match scale {
        ScopeScale::TenBit => steps(8, |i| (i * 128).min(1023)),
        ScopeScale::TwelveBit => steps(8, |i| (i * 512).min(4095)),
        ScopeScale::Percent => steps(10, |i| i * 10),
        ScopeScale::Millivolts => steps(7, |i| i * 100),
        ScopeScale::Hdr => match colors {
            Some(Hdr::Hlg) => nits_marks(&[0.0, 10.0, 100.0, 203.0, 1000.0], hlg_code),
            None | Some(Hdr::Pq) => nits_marks(&[0.0, 1.0, 10.0, 100.0, 1000.0, 10000.0], pq_code),
        },
    }
}

fn from_top(v: f64) -> f64 {
    (HIGH - v) / (HIGH - LOW)
}

fn luma_of(colors: Option<Hdr>) -> (f64, f64) {
    let matrix = match colors {
        Some(_) => Matrix::Bt2020,
        None => Matrix::Bt709,
    };
    let c = shader_colors(Colors {
        matrix,
        full_range: true,
        hdr: colors,
    });
    (c.kr, c.kb)
}

fn chroma_scales((kr, kb): (f64, f64)) -> (f64, f64) {
    (0.5 / (1.0 - kb), 0.5 / (1.0 - kr))
}

fn hue_points(colors: Option<Hdr>) -> [(&'static str, [f64; 2]); 6] {
    let (kr, kb) = luma_of(colors);
    let kg = 1.0 - kr - kb;
    let (cb, cr) = chroma_scales((kr, kb));
    [
        ("R", [BARS, 0.0, 0.0]),
        ("Mg", [BARS, 0.0, BARS]),
        ("B", [0.0, 0.0, BARS]),
        ("Cy", [0.0, BARS, BARS]),
        ("G", [0.0, BARS, 0.0]),
        ("Yl", [BARS, BARS, 0.0]),
    ]
    .map(|(name, [r, g, b])| {
        let y = kr * r + kg * g + kb * b;
        (name, [(b - y) * cb, (y - r) * cr])
    })
}

fn per_unit((w, h): (u32, u32)) -> f64 {
    f64::from(w.min(h)) / (2.0 * REACH)
}

pub fn marks(
    scope: Scope,
    (colors, scale): (Option<Hdr>, ScopeScale),
    size: (u32, u32),
) -> Vec<Mark> {
    match scope {
        Scope::Waveform | Scope::Parade => levels(scale, colors)
            .into_iter()
            .map(|(label, v)| Mark {
                label,
                at: [0.0, from_top(v) as f32],
            })
            .collect(),
        Scope::Vectorscope => {
            let unit = per_unit(size);
            hue_points(colors)
                .into_iter()
                .map(|(name, [x, y])| Mark {
                    label: name.to_string(),
                    at: [
                        (0.5 + x * unit / f64::from(size.0)) as f32,
                        (0.5 + y * unit / f64::from(size.1)) as f32,
                    ],
                })
                .collect()
        }
    }
}

fn params(
    scope: Scope,
    (colors, scale): (Option<Hdr>, ScopeScale),
    canvas: (u32, u32),
    size: (u32, u32),
) -> [[u8; 4]; PARAMS / 4] {
    let (kr, kb) = luma_of(colors);
    let (cb, cr) = chroma_scales((kr, kb));
    let unit = per_unit(size);
    let pixels = u64::from(canvas.0) * u64::from(canvas.1);
    let columns = match scope {
        Scope::Waveform => size.0,
        Scope::Parade => size.0 / 3,
        Scope::Vectorscope => size.0.min(size.1),
    };
    let full = (pixels / u64::from(columns.max(1)) / FULL_AT).max(1);
    let f = |v: f64| (v as f32).to_le_bytes();
    let n = |v: u32| v.to_le_bytes();
    let mut rows = [f(-1.0); MOST_ROWS];
    let lines = match scope {
        Scope::Vectorscope => Vec::new(),
        _ => levels(scale, colors),
    };
    for (row, (_, v)) in rows.iter_mut().zip(&lines) {
        let at = (from_top(*v) * f64::from(size.1)).floor();
        *row = f(at.min(f64::from(size.1 - 1)));
    }
    let middle = [f64::from(size.0) / 2.0, f64::from(size.1) / 2.0];
    let hues = hue_points(colors).map(|(_, [x, y])| {
        [
            f((middle[0] + x * unit).floor()),
            f((middle[1] + y * unit).floor()),
        ]
    });
    let kind = match scope {
        Scope::Waveform => 0,
        Scope::Parade => 1,
        Scope::Vectorscope => 2,
    };
    let mut bytes = vec![
        n(canvas.0),
        n(canvas.1),
        n(size.0),
        n(size.1),
        f(kr),
        f(1.0 - kr - kb),
        f(kb),
        f(0.0),
        f(cb),
        f(cr),
        f(CIRCLE * unit),
        f((f64::from(size.0.min(size.1)) / 64.0).max(3.0)),
        f(LOW),
        f(HIGH - LOW),
        f(1.0 / full as f64),
        f(unit),
    ];
    bytes.extend(rows);
    bytes.extend(hues.iter().flatten());
    bytes.extend([n(kind), n(lines.len().min(MOST_ROWS) as u32), n(0), n(0)]);
    bytes
        .try_into()
        .expect("the fields of the scope's parameters")
}

struct Plot {
    scope: Scope,
    size: (u32, u32),
    generation: u64,
    target: Target,
    params: wgpu::Buffer,
    hits: wgpu::Buffer,
    painted: wgpu::BindGroup,
    drawn: Option<u64>,
}

#[derive(Default)]
pub(super) struct Plots {
    plots: Vec<Plot>,
    made: u64,
    scale: ScopeScale,
}

impl Plots {
    fn make(gpu: &Gpu, pipes: &Pipes, scope: Scope, size: (u32, u32), generation: u64) -> Plot {
        let params = crate::compute::buffer(
            &gpu.device,
            "vidcut scope",
            PARAMS,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let hits = crate::compute::buffer(
            &gpu.device,
            "vidcut scope hits",
            size.0 as usize * size.1 as usize * 4,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let painted = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vidcut scope"),
            layout: &pipes.painting.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: hits.as_entire_binding(),
                },
            ],
        });
        Plot {
            scope,
            size,
            generation,
            target: make_target(&gpu.device, size),
            params,
            hits,
            painted,
            drawn: None,
        }
    }

    pub(super) fn paint(
        &mut self,
        gpu: &Gpu,
        (canvas, canvas_size): (&wgpu::TextureView, (u32, u32)),
        (colors, draw): (Option<Hdr>, u64),
        wanted: &[(Scope, (u32, u32))],
        scale: ScopeScale,
    ) -> bool {
        if self.scale != scale {
            self.scale = scale;
            for p in &mut self.plots {
                p.drawn = None;
            }
        }
        let pipes = gpu.scope.get_or_init(|| Pipes::new(&gpu.device));
        self.plots.retain(|p| {
            wanted
                .iter()
                .any(|&(s, z)| s == p.scope && sized(z) == p.size)
        });
        for &(scope, size) in wanted {
            let size = sized(size);
            if !self.plots.iter().any(|p| p.scope == scope) {
                self.made += 1;
                self.plots
                    .push(Self::make(gpu, pipes, scope, size, self.made));
            }
        }
        if self.plots.iter().all(|p| p.drawn == Some(draw)) {
            return false;
        }
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("vidcut scope"),
            });
        for plot in self.plots.iter_mut().filter(|p| p.drawn != Some(draw)) {
            let words = params(plot.scope, (colors, scale), canvas_size, plot.size);
            gpu.queue
                .write_buffer(&plot.params, 0, words.as_flattened());
            encoder.clear_buffer(&plot.hits, 0, None);
            let gathering = pipes.gathering(plot.scope);
            let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("vidcut scope"),
                layout: &gathering.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: plot.params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(canvas),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: plot.hits.as_entire_binding(),
                    },
                ],
            });
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(gathering);
                pass.set_bind_group(0, &bind, &[]);
                pass.dispatch_workgroups(canvas_size.0.div_ceil(16), canvas_size.1.div_ceil(16), 1);
            }
            {
                let mut pass = begin(&mut encoder, &plot.target.view, Some(wgpu::Color::BLACK));
                pass.set_pipeline(&pipes.painting);
                pass.set_bind_group(0, &plot.painted, &[]);
                pass.draw(0..3, 0..1);
            }
            plot.drawn = Some(draw);
        }
        gpu.submit([encoder.finish()]);
        true
    }

    pub(super) fn drop_all(&mut self) {
        self.plots.clear();
    }

    pub(super) fn views(&self) -> Vec<(Scope, u64, wgpu::TextureView)> {
        self.plots
            .iter()
            .map(|p| (p.scope, p.generation, p.target.view.clone()))
            .collect()
    }

    pub(super) fn target(&self, scope: Scope) -> Option<&Target> {
        self.plots
            .iter()
            .find(|p| p.scope == scope)
            .map(|p| &p.target)
    }
}

fn sized((w, h): (u32, u32)) -> (u32, u32) {
    (w.clamp(SMALLEST, LARGEST), h.clamp(SMALLEST, LARGEST))
}
