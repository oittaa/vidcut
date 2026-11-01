use vidcut_core::place::{Blurring, Placing, Soft};

use super::{Gpu, MIXING, begin};

const MOST: f64 = 128.0;

const OPEN: f64 = 1e6;

const REACH: f64 = 4.0;

const LEFT: f64 = 1e-3;

const PARAMS: usize = 48;

pub(super) struct Pipes {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
}

impl Pipes {
    pub(super) fn new(device: &wgpu::Device, shader: &wgpu::ShaderModule) -> Self {
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vidcut blur"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture(1),
                texture(2),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vidcut blur"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vidcut blur"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs_blur"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: MIXING,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self { layout, pipeline }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Pass {
    spread: [f64; 4],
    step: u32,
    taps: u32,
}

fn passes(want: [f64; 4]) -> Vec<Pass> {
    let mut done = [0.0_f64; 4];
    let mut out: Vec<Pass> = Vec::new();
    loop {
        let left: [f64; 4] = std::array::from_fn(|c| (want[c] * want[c] - done[c]).max(0.0).sqrt());
        let open: Vec<usize> = (0..4).filter(|&c| left[c] > LEFT).collect();
        if open.is_empty() {
            return out;
        }
        let reached = open
            .iter()
            .map(|&c| done[c].sqrt())
            .fold(f64::INFINITY, f64::min);
        let rest = open.iter().map(|&c| left[c]).fold(0.0, f64::max);
        let step = match out.is_empty() {
            true => 1.0,
            false => (rest / MOST).ceil().min(reached.floor()).max(1.0),
        };
        let spread = left.map(|l| l.min(MOST * step));
        for (d, s) in done.iter_mut().zip(spread) {
            *d += s * s;
        }
        let most = spread.into_iter().fold(0.0, f64::max);
        out.push(Pass {
            spread,
            step: step as u32,
            taps: (REACH * most / step).ceil() as u32,
        });
    }
}

fn bytes(pass: &Pass, down: bool, blur: &Blurring, (first, last): (bool, bool)) -> Vec<u8> {
    let mut out = Vec::with_capacity(PARAMS);
    for v in pass.spread {
        out.extend_from_slice(&(v as f32).to_le_bytes());
    }
    let border = blur.border.code();
    for v in [u32::from(down), pass.step, pass.taps, border] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let use_alpha = if blur.use_alpha { 1.0 } else { 0.0 };
    let first = if first { 1.0 } else { 0.0 };
    let last = if last { 1.0 } else { 0.0 };
    for v in [blur.blend, use_alpha, last, first] {
        out.extend_from_slice(&(v as f32).to_le_bytes());
    }
    out
}

struct Texture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Texture {
    fn new(gpu: &Gpu, size: (u32, u32)) -> Self {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vidcut blur"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: MIXING,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self { texture, view }
    }
}

#[derive(Default)]
pub(super) struct Blurred {
    size: (u32, u32),
    scratch: Option<[Texture; 2]>,
    layers: Vec<Option<Texture>>,
    masks: Vec<Option<Texture>>,
}

impl Blurred {
    pub(super) fn sized(&mut self, size: (u32, u32), n: usize) {
        if self.size != size {
            self.size = size;
            self.scratch = None;
            self.layers.clear();
            self.masks.clear();
        }
        self.layers.resize_with(n, || None);
        self.masks.resize_with(n, || None);
    }

    pub(super) fn drop_layer(&mut self, i: usize) {
        for t in [self.layers.get_mut(i), self.masks.get_mut(i)]
            .into_iter()
            .flatten()
        {
            *t = None;
        }
    }

    pub(super) fn layer(
        &mut self,
        gpu: &Gpu,
        i: usize,
        drawn: &wgpu::BindGroup,
        effects: &[Blurring],
        encoder: &mut wgpu::CommandEncoder,
    ) -> &wgpu::TextureView {
        let size = self.size;
        self.masks[i] = None;
        let own = self.layers[i].get_or_insert_with(|| Texture::new(gpu, size));
        add(gpu, &own.view, drawn, encoder);
        let scratch = self
            .scratch
            .get_or_insert_with(|| [Texture::new(gpu, size), Texture::new(gpu, size)]);
        blur_into(gpu, own, scratch, effects, encoder);
        &own.view
    }

    pub(super) fn made(
        &mut self,
        gpu: &Gpu,
        i: usize,
        (drawn, placing): (&wgpu::BindGroup, &Placing),
        (effects, live): (&[Blurring], &[Blurring]),
        encoder: &mut wgpu::CommandEncoder,
    ) -> (&wgpu::TextureView, &wgpu::TextureView, f64) {
        let size = self.size;
        let own = self.layers[i].get_or_insert_with(|| Texture::new(gpu, size));
        add(gpu, &own.view, drawn, encoder);
        let mask = self.masks[i].get_or_insert_with(|| Texture::new(gpu, size));
        add(
            gpu,
            &mask.view,
            &mask_bind(gpu, &own.view, placing),
            encoder,
        );
        let scratch = self
            .scratch
            .get_or_insert_with(|| [Texture::new(gpu, size), Texture::new(gpu, size)]);
        blur_into(gpu, mask, scratch, effects, encoder);
        if live.is_empty() {
            return (&own.view, &mask.view, 5.0);
        }
        let joined = shown_bind(
            gpu,
            (&own.view, &mask.view),
            [1.0, 1.0, 0.0, 0.0],
            [5.0, 0.0, 0.0, 0.0],
        );
        add(gpu, &scratch[0].view, &joined, encoder);
        encoder.copy_texture_to_texture(
            scratch[0].texture.as_image_copy(),
            own.texture.as_image_copy(),
            own.texture.size(),
        );
        blur_into(gpu, own, scratch, live, encoder);
        (&own.view, &own.view, 3.0)
    }
}

fn add(
    gpu: &Gpu,
    to: &wgpu::TextureView,
    drawn: &wgpu::BindGroup,
    encoder: &mut wgpu::CommandEncoder,
) {
    let mut pass = begin(encoder, to, Some(wgpu::Color::TRANSPARENT));
    pass.set_pipeline(&gpu.adding);
    pass.set_bind_group(0, drawn, &[]);
    pass.draw(0..3, 0..1);
}

fn blur_into(
    gpu: &Gpu,
    own: &Texture,
    scratch: &[Texture; 2],
    effects: &[Blurring],
    encoder: &mut wgpu::CommandEncoder,
) {
    let pipes = &gpu.blur;
    for effect in effects {
        let mut ways: Vec<(Pass, bool)> = passes(effect.across)
            .into_iter()
            .map(|p| (p, false))
            .chain(passes(effect.down).into_iter().map(|p| (p, true)))
            .collect();
        if ways.is_empty() {
            let still = Pass {
                spread: [0.0; 4],
                step: 1,
                taps: 0,
            };
            ways.push((still, false));
        }
        let mut from = &own.view;
        let mut k = 0;
        for (j, (pass, down)) in ways.iter().enumerate() {
            let to = &scratch[k];
            let params = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("vidcut blur"),
                size: PARAMS as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let last = j + 1 == ways.len();
            gpu.queue
                .write_buffer(&params, 0, &bytes(pass, *down, effect, (j == 0, last)));
            let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("vidcut blur"),
                layout: &pipes.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(from),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&own.view),
                    },
                ],
            });
            let mut drawing = begin(encoder, &to.view, None);
            drawing.set_pipeline(&pipes.pipeline);
            drawing.set_bind_group(0, &bind, &[]);
            drawing.draw(0..3, 0..1);
            drop(drawing);
            from = &to.view;
            k = 1 - k;
        }
        let made = &scratch[1 - k];
        encoder.copy_texture_to_texture(
            made.texture.as_image_copy(),
            own.texture.as_image_copy(),
            own.texture.size(),
        );
    }
}

pub(super) fn opened(placing: &Placing) -> Placing {
    let mut out = placing.clone();
    for l in &mut out.levels {
        l.edge = [-OPEN, -OPEN, OPEN, OPEN];
        l.soft = Soft::default();
    }
    out
}

fn mask_bind(gpu: &Gpu, view: &wgpu::TextureView, placing: &Placing) -> wgpu::BindGroup {
    let inner = super::innermost(Some(placing));
    let levels = super::level_floats(inner);
    let mut floats = vec![0.0_f64; super::PARAMS / 4 - levels.len()];
    floats[40..45].copy_from_slice(&[inner.len() as f64, 1.0, 0.0, 0.0, 4.0]);
    floats.extend(levels);
    picture_bind(gpu, &floats, (view, view))
}

pub(super) fn shown_bind(
    gpu: &Gpu,
    views: (&wgpu::TextureView, &wgpu::TextureView),
    placed: [f64; 4],
    made: [f64; 4],
) -> wgpu::BindGroup {
    let mut floats = vec![0.0_f64; super::PARAMS / 4];
    floats[40..44].copy_from_slice(&placed);
    floats[44..48].copy_from_slice(&made);
    picture_bind(gpu, &floats, views)
}

fn picture_bind(
    gpu: &Gpu,
    floats: &[f64],
    (color, alpha): (&wgpu::TextureView, &wgpu::TextureView),
) -> wgpu::BindGroup {
    let bytes: Vec<u8> = floats
        .iter()
        .flat_map(|v| (*v as f32).to_le_bytes())
        .collect();
    let params = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vidcut blurred"),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    gpu.queue.write_buffer(&params, 0, &bytes);
    let texture = |binding, view| wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    };
    gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("vidcut blurred"),
        layout: &gpu.layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: params.as_entire_binding(),
            },
            texture(1, color),
            texture(2, color),
            texture(3, color),
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(&gpu.sampler),
            },
            texture(7, color),
            texture(8, alpha),
        ],
    })
}
