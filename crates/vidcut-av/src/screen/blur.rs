use vidcut_core::place::{Blurring, Level, Placing, Soft, Tiling, Work, Working};

use super::{Gpu, MIXING, begin};

const KERNEL: usize = 35;

const MOST: f64 = 128.0;

const OPEN: f64 = 1e6;

const REACH: f64 = 4.0;

const LEFT: f64 = 1e-3;

const PARAMS: usize = 48;

const MOSAIC_STAGES: u32 = 3;

pub(super) struct Pipes {
    layout: wgpu::BindGroupLayout,
    blur: wgpu::RenderPipeline,
    mosaic: wgpu::RenderPipeline,
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
        let pipeline = |entry: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
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
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: MIXING,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        Self {
            blur: pipeline("fs_blur"),
            mosaic: pipeline("fs_mosaic"),
            layout,
        }
    }
}

struct Step<'a> {
    pipeline: &'a wgpu::RenderPipeline,
    params: Vec<u8>,
}

fn steps<'a>(pipes: &'a Pipes, effect: &Working) -> Vec<Step<'a>> {
    match &effect.work {
        Work::Blur(blur) => blur_steps(&pipes.blur, blur, effect),
        Work::Mosaic(tiling) => (0..MOSAIC_STAGES)
            .map(|stage| Step {
                pipeline: &pipes.mosaic,
                params: mosaic_bytes(tiling, effect, stage),
            })
            .collect(),
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

fn flag(on: bool) -> f64 {
    if on { 1.0 } else { 0.0 }
}

fn blur_bytes(
    pass: &Pass,
    down: bool,
    (blur, effect): (&Blurring, &Working),
    (first, last): (bool, bool),
) -> Vec<u8> {
    let mut out = float_bytes(&pass.spread);
    for v in [u32::from(down), pass.step, pass.taps, blur.border.code()] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let more = [
        effect.blend,
        flag(effect.use_alpha),
        flag(last),
        flag(first),
    ];
    out.extend(float_bytes(&more));
    out
}

fn blur_steps<'a>(
    pipeline: &'a wgpu::RenderPipeline,
    blur: &Blurring,
    effect: &Working,
) -> Vec<Step<'a>> {
    let mut ways: Vec<(Pass, bool)> = passes(blur.across)
        .into_iter()
        .map(|p| (p, false))
        .chain(passes(blur.down).into_iter().map(|p| (p, true)))
        .collect();
    if ways.is_empty() {
        let still = Pass {
            spread: [0.0; 4],
            step: 1,
            taps: 0,
        };
        ways.push((still, false));
    }
    let count = ways.len();
    ways.iter()
        .enumerate()
        .map(|(j, (pass, down))| Step {
            pipeline,
            params: blur_bytes(pass, *down, (blur, effect), (j == 0, j + 1 == count)),
        })
        .collect()
}

fn mosaic_bytes(tiling: &Tiling, effect: &Working, stage: u32) -> Vec<u8> {
    let [cw, ch] = tiling.cell;
    let [ox, oy] = tiling.origin;
    let mut out = float_bytes(&[cw, ch, ox, oy]);
    let [rx, ry] = tiling.reach;
    for v in [rx, ry, tiling.shape.code(), tiling.samples] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let more = [
        tiling.step,
        effect.blend,
        flag(effect.use_alpha),
        f64::from(stage),
    ];
    out.extend(float_bytes(&more));
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
        effects: &[Working],
        encoder: &mut wgpu::CommandEncoder,
    ) -> &wgpu::TextureView {
        let size = self.size;
        self.masks[i] = None;
        let own = self.layers[i].get_or_insert_with(|| Texture::new(gpu, size));
        add(gpu, &own.view, drawn, encoder);
        let scratch = self
            .scratch
            .get_or_insert_with(|| [Texture::new(gpu, size), Texture::new(gpu, size)]);
        effects_into(gpu, own, scratch, effects, encoder);
        &own.view
    }

    pub(super) fn made(
        &mut self,
        gpu: &Gpu,
        i: usize,
        (drawn, placing): (&wgpu::BindGroup, &Placing),
        (effects, live): (&[Working], &[Working]),
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
        effects_into(gpu, mask, scratch, effects, encoder);
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
        effects_into(gpu, own, scratch, live, encoder);
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

fn effects_into(
    gpu: &Gpu,
    own: &Texture,
    scratch: &[Texture; 2],
    effects: &[Working],
    encoder: &mut wgpu::CommandEncoder,
) {
    let pipes = &gpu.blur;
    for effect in effects {
        let mut from = &own.view;
        let mut k = 0;
        for step in steps(pipes, effect) {
            let to = &scratch[k];
            let params = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("vidcut blur"),
                size: PARAMS as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            gpu.queue.write_buffer(&params, 0, &step.params);
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
            drawing.set_pipeline(step.pipeline);
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

pub(super) fn whole_bind(
    gpu: &Gpu,
    view: &wgpu::TextureView,
    (w, h): (u32, u32),
    level: Level,
    kernel: u32,
) -> wgpu::BindGroup {
    let levels = super::level_floats(&[level]);
    let mut floats = vec![0.0_f64; super::PARAMS / 4 - levels.len()];
    floats[4..6].copy_from_slice(&[f64::from(w), f64::from(h)]);
    floats[40..42].copy_from_slice(&[1.0, 1.0]);
    floats.extend(levels);
    let mut bytes = float_bytes(&floats);
    bytes[KERNEL * 4..(KERNEL + 1) * 4].copy_from_slice(&kernel.to_le_bytes());
    bytes_bind(gpu, &bytes, (view, view))
}

fn float_bytes(floats: &[f64]) -> Vec<u8> {
    floats
        .iter()
        .flat_map(|v| (*v as f32).to_le_bytes())
        .collect()
}

fn picture_bind(
    gpu: &Gpu,
    floats: &[f64],
    views: (&wgpu::TextureView, &wgpu::TextureView),
) -> wgpu::BindGroup {
    bytes_bind(gpu, &float_bytes(floats), views)
}

fn bytes_bind(
    gpu: &Gpu,
    bytes: &[u8],
    (color, alpha): (&wgpu::TextureView, &wgpu::TextureView),
) -> wgpu::BindGroup {
    let params = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vidcut blurred"),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    gpu.queue.write_buffer(&params, 0, bytes);
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
