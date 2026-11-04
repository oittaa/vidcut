use vidcut_core::lru::Lru;
use vidcut_core::model::GlowOutput;
use vidcut_core::place::{
    Blurring, GlowSpread, Glowing, Level, Placing, Soft, Tiling, Work, Working,
};

use super::{Gpu, MIXING, begin};

const KERNEL: usize = 35;

const MOST: f64 = 128.0;

const OPEN: f64 = 1e6;

const REACH: f64 = 4.0;

const LEFT: f64 = 1e-3;

const MOSAIC_STAGES: u32 = 3;

const GLOW_STEPS: [u32; 3] = [8, 4, 2];

const GLOW_FINE: f64 = 8.0;

const TAIL: (f64, f64) = (3.8, 4.3);

const BORDER_BLACK: u32 = 0;

const BORDER_MIRROR: u32 = 2;

const FLAG_INVERT: i32 = 1;

const FLAG_SEEN: i32 = 2;

const FLAG_SECOND: i32 = 4;

const FLAG_NOISE: i32 = 8;

const SPARE_TEXTURES: usize = 6;

pub(super) struct Pipes {
    layout: wgpu::BindGroupLayout,
    blur: wgpu::RenderPipeline,
    mosaic: wgpu::RenderPipeline,
    glow_mask: wgpu::RenderPipeline,
    glow_cells: wgpu::RenderPipeline,
    glow: wgpu::RenderPipeline,
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
                texture(14),
                texture(15),
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
            glow_mask: pipeline("fs_glow_mask"),
            glow_cells: pipeline("fs_glow_cells"),
            glow: pipeline("fs_glow"),
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
        Work::Glow(_) => Vec::new(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Pass {
    spread: [f64; 4],
    step: u32,
    taps: u32,
}

fn passes(want: [f64; 4]) -> Vec<Pass> {
    passes_within(want, MOST, REACH)
}

fn passes_within(want: [f64; 4], most: f64, reach: f64) -> Vec<Pass> {
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
            false => (rest / most).ceil().min(reached.floor()).max(1.0),
        };
        let spread = left.map(|l| l.min(most * step));
        for (d, s) in done.iter_mut().zip(spread) {
            *d += s * s;
        }
        let widest = spread.into_iter().fold(0.0, f64::max);
        out.push(Pass {
            spread,
            step: step as u32,
            taps: (reach * widest / step).ceil() as u32,
        });
    }
}

fn flag(on: bool) -> f64 {
    if on { 1.0 } else { 0.0 }
}

fn pass_bytes(pass: &Pass, down: bool, border: u32, more: [f64; 4], window: [f64; 4]) -> Vec<u8> {
    let mut out = float_bytes(&pass.spread);
    for v in [u32::from(down), pass.step, pass.taps, border] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend(float_bytes(&more));
    out.extend(float_bytes(&window));
    out
}

fn blur_bytes(
    pass: &Pass,
    down: bool,
    (blur, effect): (&Blurring, &Working),
    (first, last): (bool, bool),
) -> Vec<u8> {
    let more = [
        effect.blend,
        flag(effect.use_alpha),
        flag(last),
        flag(first),
    ];
    pass_bytes(pass, down, blur.border.code(), more, [0.0; 4])
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
    let keeps_alpha = effect.use_alpha && !tiling.blur_alpha;
    let whole = effect.use_alpha && tiling.blur_alpha && tiling.whole;
    out.extend(float_bytes(&[flag(keeps_alpha), flag(whole), 0.0, 0.0]));
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct GlowGrid {
    step: [u32; 2],
    nodes: [u32; 2],
    last: [i32; 2],
    low: [i32; 2],
    across: [f64; 4],
    down: [f64; 4],
    still: [f64; 4],
}

fn glow_step(spreads: &[f64; 3]) -> u32 {
    let least = spreads
        .iter()
        .copied()
        .filter(|s| *s > 0.0)
        .fold(f64::INFINITY, f64::min);
    GLOW_STEPS
        .into_iter()
        .find(|d| least.is_finite() && least >= GLOW_FINE * f64::from(*d))
        .unwrap_or(1)
}

fn glow_low(spread: f64, step: u32) -> f64 {
    if spread <= 0.0 {
        return 0.0;
    }
    let d = f64::from(step);
    (spread * spread - (d * d - 1.0) / 6.0).max(0.0).sqrt() / d
}

impl GlowGrid {
    fn of(spread: &GlowSpread, (w, h): (u32, u32), seen: bool) -> Self {
        let step = [glow_step(&spread.across), glow_step(&spread.down)];
        let [gx, gy] = spread.grid;
        let whole = |n: u32, grid: u32| n.div_ceil(grid) * grid;
        let nodes =
            |n: u32, d: u32, grid: u32| (whole(n, grid) + if seen { grid } else { 0 }) / d + 1;
        let lows =
            |s: &[f64; 3], d: u32| [glow_low(s[0], d), glow_low(s[1], d), glow_low(s[2], d), 0.0];
        let still = std::array::from_fn(|c| {
            flag(c < 3 && spread.across[c.min(2)] <= 0.0 && spread.down[c.min(2)] <= 0.0)
        });
        Self {
            step,
            nodes: [nodes(w, step[0], gx), nodes(h, step[1], gy)],
            last: [
                whole(w, gx) as i32 - (gx / 2) as i32,
                whole(h, gy) as i32 - (gy / 2) as i32,
            ],
            low: [-((gx / 2) as i32), -((gy / 2) as i32)],
            across: lows(&spread.across, step[0]),
            down: lows(&spread.down, step[1]),
            still,
        }
    }

    fn cells_bytes(&self, seen: bool) -> Vec<u8> {
        let [dx, dy] = self.step;
        let mut out = Vec::new();
        for v in [dx as i32, dy as i32, i32::from(seen), 0] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in [self.low[0], self.low[1], self.last[0], self.last[1]] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    fn sampled(&self) -> [f64; 4] {
        [
            f64::from(self.step[0]),
            f64::from(self.step[1]),
            f64::from(self.low[0]),
            f64::from(self.low[1]),
        ]
    }

    fn ways(&self) -> Vec<(Pass, bool)> {
        let most = MOST * f64::from(self.step[0] * self.step[1]);
        passes_within(self.across, most, TAIL.1)
            .into_iter()
            .map(|p| (p, false))
            .chain(
                passes_within(self.down, most, TAIL.1)
                    .into_iter()
                    .map(|p| (p, true)),
            )
            .collect()
    }

    fn blurs(&self) -> bool {
        self.still[..3].contains(&0.0)
    }
}

fn glow_bytes(
    glow: &Glowing,
    effect: &Working,
    (one, two): (&GlowGrid, Option<&GlowGrid>),
) -> Vec<u8> {
    let [fr, fg, fb] = glow.filter;
    let [sr, sg, sb] = glow.second_color;
    let mut out = float_bytes(&[glow.shine, glow.alpha_shine, glow.gain, glow.second_gain]);
    out.extend(float_bytes(&[fr, fg, fb, glow.gamma]));
    out.extend(float_bytes(&[sr, sg, sb, glow.saturation]));
    out.extend(float_bytes(&[
        glow.recovery,
        glow.opacity,
        effect.blend,
        flag(effect.use_alpha),
    ]));
    let flags = i32::from(glow.invert) * FLAG_INVERT
        + i32::from(glow.seen_only) * FLAG_SEEN
        + i32::from(two.is_some()) * FLAG_SECOND
        + i32::from(glow.noise.is_some()) * FLAG_NOISE;
    for v in [
        glow.output.code() as i32,
        glow.alpha.code() as i32,
        glow.mode.code() as i32,
        flags,
    ] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend(float_bytes(&one.sampled()));
    out.extend(float_bytes(
        &two.map_or([1.0, 1.0, 0.0, 0.0], GlowGrid::sampled),
    ));
    out.extend(float_bytes(&one.still));
    out.extend(float_bytes(&two.map_or([1.0; 4], |g| g.still)));
    let (noise, depth) = glow.noise.map_or(([0.0; 4], 0.0), |n| {
        (
            [n.strength, n.detail, 1.0 / n.cell[0], 1.0 / n.cell[1]],
            n.depth,
        )
    });
    out.extend(float_bytes(&noise));
    out.extend(float_bytes(&[depth, 0.0, 0.0, 0.0]));
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

type Spare = Lru<(u32, u32, u32), Texture, SPARE_TEXTURES>;

fn spare_ready(gpu: &Gpu, spare: &mut Spare, (w, h): (u32, u32), slots: [u32; 2]) {
    for slot in slots {
        let key = (w, h, slot);
        if spare.get(&key).is_none() {
            spare.insert(key, Texture::new(gpu, (w, h)));
        }
    }
}

#[derive(Default)]
pub(super) struct Blurred {
    size: (u32, u32),
    scratch: Option<[Texture; 2]>,
    layers: Vec<Option<Texture>>,
    masks: Vec<Option<Texture>>,
    spare: Spare,
}

impl Blurred {
    pub(super) fn sized(&mut self, size: (u32, u32), n: usize) {
        if self.size != size {
            self.size = size;
            self.scratch = None;
            self.layers.clear();
            self.masks.clear();
            self.spare.clear();
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
        effects_into(gpu, own, (scratch, &mut self.spare), effects, encoder);
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
        effects_into(gpu, mask, (scratch, &mut self.spare), effects, encoder);
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
        effects_into(gpu, own, (scratch, &mut self.spare), live, encoder);
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

struct Views<'a> {
    source: &'a wgpu::TextureView,
    start: &'a wgpu::TextureView,
    second: &'a wgpu::TextureView,
    mask: &'a wgpu::TextureView,
}

impl<'a> Views<'a> {
    fn same(view: &'a wgpu::TextureView) -> Self {
        Self::with(view, view)
    }

    fn with(source: &'a wgpu::TextureView, start: &'a wgpu::TextureView) -> Self {
        Self {
            source,
            start,
            second: start,
            mask: start,
        }
    }
}

fn draw_step(
    gpu: &Gpu,
    (pipeline, bytes): (&wgpu::RenderPipeline, &[u8]),
    views: Views<'_>,
    to: &wgpu::TextureView,
    encoder: &mut wgpu::CommandEncoder,
) {
    let params = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vidcut blur"),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    gpu.queue.write_buffer(&params, 0, bytes);
    let texture = |binding, view| wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    };
    let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("vidcut blur"),
        layout: &gpu.blur.layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: params.as_entire_binding(),
            },
            texture(1, views.source),
            texture(2, views.start),
            texture(14, views.second),
            texture(15, views.mask),
        ],
    });
    let mut drawing = begin(encoder, to, None);
    drawing.set_pipeline(pipeline);
    drawing.set_bind_group(0, &bind, &[]);
    drawing.draw(0..3, 0..1);
}

fn glow_blurred<'a>(
    gpu: &Gpu,
    grid: &GlowGrid,
    seen: bool,
    (mask, spare, slots): (&'a wgpu::TextureView, &'a Spare, [u32; 2]),
    encoder: &mut wgpu::CommandEncoder,
) -> &'a wgpu::TextureView {
    let (w, h) = (grid.nodes[0], grid.nodes[1]);
    let textures = slots.map(|slot| spare.peek(&(w, h, slot)));
    let [Some(a), Some(b)] = textures else {
        return mask;
    };
    let cells = (&gpu.blur.glow_cells, &grid.cells_bytes(seen)[..]);
    draw_step(gpu, cells, Views::same(mask), &a.view, encoder);
    let border = if seen { BORDER_BLACK } else { BORDER_MIRROR };
    let window = [1.0, TAIL.0, TAIL.1, 0.0];
    let (mut from, mut to) = (a, b);
    for (pass, down) in grid.ways() {
        let bytes = pass_bytes(&pass, down, border, [0.0; 4], window);
        let step = (&gpu.blur.blur, &bytes[..]);
        draw_step(gpu, step, Views::same(&from.view), &to.view, encoder);
        std::mem::swap(&mut from, &mut to);
    }
    &from.view
}

fn glow_into(
    gpu: &Gpu,
    own: &Texture,
    (scratch, spare): (&[Texture; 2], &mut Spare),
    (glow, effect): (&Glowing, &Working),
    encoder: &mut wgpu::CommandEncoder,
) {
    let size = own.texture.size();
    let size = (size.width, size.height);
    let seen = glow.seen_only;
    let one = GlowGrid::of(&glow.first, size, seen);
    let two = glow.second.as_ref().map(|s| GlowGrid::of(s, size, seen));
    let bytes = glow_bytes(glow, effect, (&one, two.as_ref()));
    let mask = &scratch[0].view;
    let first = (&gpu.blur.glow_mask, &bytes[..]);
    draw_step(gpu, first, Views::same(&own.view), mask, encoder);
    let blurs = |g: &GlowGrid| glow.output != GlowOutput::ShinyRegions && g.blurs();
    let one_blurs = blurs(&one).then_some(&one);
    let two_blurs = two.as_ref().filter(|g| blurs(g));
    for (g, slots) in [(one_blurs, [0, 1]), (two_blurs, [2, 3])] {
        if let Some(g) = g {
            spare_ready(gpu, spare, (g.nodes[0], g.nodes[1]), slots);
        }
    }
    let spare = &*spare;
    let mut blurred = |g: Option<&GlowGrid>, slots| match g {
        Some(g) => glow_blurred(gpu, g, seen, (mask, spare, slots), encoder),
        None => mask,
    };
    let views = Views {
        source: blurred(one_blurs, [0, 1]),
        start: &own.view,
        second: blurred(two_blurs, [2, 3]),
        mask,
    };
    let made = &scratch[1];
    draw_step(
        gpu,
        (&gpu.blur.glow, &bytes[..]),
        views,
        &made.view,
        encoder,
    );
    encoder.copy_texture_to_texture(
        made.texture.as_image_copy(),
        own.texture.as_image_copy(),
        own.texture.size(),
    );
}

fn effects_into(
    gpu: &Gpu,
    own: &Texture,
    (scratch, spare): (&[Texture; 2], &mut Spare),
    effects: &[Working],
    encoder: &mut wgpu::CommandEncoder,
) {
    let pipes = &gpu.blur;
    for effect in effects {
        if let Work::Glow(glow) = &effect.work {
            glow_into(gpu, own, (scratch, spare), (glow.as_ref(), effect), encoder);
            continue;
        }
        let mut from = &own.view;
        let mut k = 0;
        for step in steps(pipes, effect) {
            let to = &scratch[k];
            let views = Views::with(from, &own.view);
            draw_step(
                gpu,
                (step.pipeline, &step.params[..]),
                views,
                &to.view,
                encoder,
            );
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
