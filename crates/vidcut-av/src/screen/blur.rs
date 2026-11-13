use vidcut_core::lru::Lru;
use vidcut_core::model::{AreaShape, GlowOutput};
use vidcut_core::place::{
    Blurring, Boxing, Figure, GlowSpread, Glowing, GrainTile, Graining, Level, Masking, Outline,
    Placing, Shading, Shaping, Sharpening, Softening, Tiling, Work, Working,
};

use super::{Gpu, MIXING, begin, grain, sharpen, vignette};

const KERNEL: usize = 35;

const MOST: f64 = 128.0;

pub(super) const REACH: f64 = 4.0;

const LEFT: f64 = 1e-3;

const MOSAIC_STAGES: u32 = 3;

const GLOW_STEPS: [u32; 3] = [8, 4, 2];

const GLOW_FINE: f64 = 8.0;

const TAIL: (f64, f64) = (3.8, 4.3);

const BORDER_BLACK: u32 = 0;

const BORDER_CLAMP: u32 = 1;

const MOST_BOX: f64 = 65536.0;

const BORDER_MIRROR: u32 = 2;

const BORDER_HALF: u32 = 4;

const FLAG_INVERT: i32 = 1;

const FLAG_SEEN: i32 = 2;

const FLAG_SECOND: i32 = 4;

const FLAG_NOISE: i32 = 8;

const SPARE_TEXTURES: usize = 6;

const GRAIN_TILES: usize = 2;

const GRAIN_ONLY: i32 = 1;

const GRAIN_DETAIL: i32 = 2;

const DETAIL_SLOTS: [u32; 2] = [4, 5];

pub(super) struct Pipes {
    layout: wgpu::BindGroupLayout,
    reading: wgpu::BindGroupLayout,
    blur: wgpu::RenderPipeline,
    mosaic: wgpu::RenderPipeline,
    vignette: wgpu::RenderPipeline,
    sharpen_taken: wgpu::RenderPipeline,
    sharpen: wgpu::RenderPipeline,
    glow_mask: wgpu::RenderPipeline,
    glow_cells: wgpu::RenderPipeline,
    glow: wgpu::RenderPipeline,
    grain_noise: wgpu::RenderPipeline,
    grain: wgpu::RenderPipeline,
    area: wgpu::RenderPipeline,
    boxes: wgpu::RenderPipeline,
    shape: wgpu::RenderPipeline,
    mask_join: wgpu::RenderPipeline,
    masked: wgpu::RenderPipeline,
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
        let buffer = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let params = buffer(0, wgpu::BufferBindingType::Uniform);
        let views = [texture(1), texture(2), texture(14), texture(15)];
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vidcut blur"),
            entries: &[params, views[0], views[1], views[2], views[3]],
        });
        let read = wgpu::BufferBindingType::Storage { read_only: true };
        let reading = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vidcut blur"),
            entries: &[
                params,
                views[0],
                views[1],
                views[2],
                views[3],
                buffer(3, read),
            ],
        });
        let of = |layout: &wgpu::BindGroupLayout| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("vidcut blur"),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            })
        };
        let pipeline = |layout: &wgpu::PipelineLayout, entry: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(layout),
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
        let plain = of(&layout);
        Self {
            blur: pipeline(&plain, "fs_blur"),
            mosaic: pipeline(&plain, "fs_mosaic"),
            vignette: pipeline(&of(&reading), "fs_vignette"),
            sharpen_taken: pipeline(&plain, "fs_sharpen_taken"),
            sharpen: pipeline(&of(&reading), "fs_sharpen"),
            glow_mask: pipeline(&plain, "fs_glow_mask"),
            glow_cells: pipeline(&plain, "fs_glow_cells"),
            glow: pipeline(&plain, "fs_glow"),
            grain_noise: pipeline(&plain, "fs_grain_noise"),
            grain: pipeline(&plain, "fs_grain"),
            area: pipeline(&of(&reading), "fs_area"),
            boxes: pipeline(&plain, "fs_boxes"),
            shape: pipeline(&of(&reading), "fs_shape"),
            mask_join: pipeline(&plain, "fs_mask_join"),
            masked: pipeline(&plain, "fs_masked"),
            layout,
            reading,
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
        Work::Boxes(boxing) => box_steps(&pipes.boxes, boxing, effect),
        Work::Mosaic(tiling) => (0..MOSAIC_STAGES)
            .map(|stage| Step {
                pipeline: &pipes.mosaic,
                params: mosaic_bytes(tiling, effect, stage),
            })
            .collect(),
        Work::Vignette(shading) => vec![Step {
            pipeline: &pipes.vignette,
            params: vignette_bytes(shading, effect),
        }],
        Work::Sharpen(sharpening) => vec![Step {
            pipeline: &pipes.sharpen,
            params: sharpen_bytes(sharpening, effect),
        }],
        Work::Glow(_) | Work::Grain(_) | Work::Clear => Vec::new(),
    }
}

fn box_passes(width: f64) -> Vec<Pass> {
    let width = width.min(MOST_BOX);
    if width <= 1.0 {
        return Vec::new();
    }
    let pass = |width: f64, step: u32, taps: u32| Pass {
        spread: [width, 0.0, 0.0, 0.0],
        step,
        taps,
    };
    if width <= 2.0 * MOST + 1.0 {
        return vec![pass(width, 1, (width / 2.0).ceil() as u32)];
    }
    let step = 2 * (width / (4.0 * MOST)).ceil() as u32 + 1;
    let s = f64::from(step);
    vec![
        pass(s, 1, step / 2),
        pass(width, step, ((width + s) / (2.0 * s)).ceil() as u32),
    ]
}

fn box_ways(across: f64, down: f64, count: u32) -> Vec<(Pass, bool)> {
    let mut ways = Vec::new();
    for _ in 0..count {
        ways.extend(box_passes(across).into_iter().map(|p| (p, false)));
        ways.extend(box_passes(down).into_iter().map(|p| (p, true)));
    }
    ways
}

fn box_steps<'a>(
    pipeline: &'a wgpu::RenderPipeline,
    boxing: &Boxing,
    effect: &Working,
) -> Vec<Step<'a>> {
    let mut ways = box_ways(boxing.across, boxing.down, boxing.count);
    if ways.is_empty() {
        let still = Pass {
            spread: [0.0; 4],
            step: 1,
            taps: 0,
        };
        ways.push((still, false));
    }
    let count = ways.len();
    let channels = boxing.channels.map(flag);
    ways.iter()
        .enumerate()
        .map(|(j, (pass, down))| {
            let more = [
                effect.blend,
                flag(effect.use_alpha),
                flag(j + 1 == count),
                flag(j == 0),
            ];
            Step {
                pipeline,
                params: pass_bytes(pass, *down, boxing.border.code(), more, channels),
            }
        })
        .collect()
}

fn soft_steps(pipes: &Pipes, soft: Option<Softening>) -> Vec<(&wgpu::RenderPipeline, Vec<u8>)> {
    let plain = [0.0; 4];
    match soft {
        None => Vec::new(),
        Some(Softening::Gaussian(sigma)) => gaussian_ways(sigma)
            .into_iter()
            .map(|(pass, down)| {
                let bytes = pass_bytes(&pass, down, BORDER_CLAMP, plain, plain);
                (&pipes.blur, bytes)
            })
            .collect(),
        Some(Softening::Boxes { width, count }) => box_ways(width, width, count)
            .into_iter()
            .map(|(pass, down)| {
                let bytes = pass_bytes(&pass, down, BORDER_CLAMP, plain, [1.0; 4]);
                (&pipes.boxes, bytes)
            })
            .collect(),
    }
}

fn shape_bytes(shape: &Shaping, figure: &Figure, joined: bool) -> Vec<u8> {
    let (kind, half, turn, corner, count, winding) = match &figure.outline {
        Outline::Ellipse { half, turn } => (0.0, *half, *turn, 0.0, 0, false),
        Outline::Rectangle { half, turn, corner } => (1.0, *half, *turn, *corner, 0, false),
        Outline::Polygon { points, winding } => (2.0, [0.0; 2], 0.0, 0.0, points.len(), *winding),
    };
    let [mx, my] = figure.middle;
    float_bytes(&[
        mx,
        my,
        turn,
        kind,
        half[0],
        half[1],
        corner,
        count as f64,
        shape.border,
        flag(shape.solid),
        flag(winding),
        flag(joined),
    ])
}

fn shape_points(gpu: &Gpu, figure: &Figure) -> wgpu::Buffer {
    let mut floats: Vec<f64> = match &figure.outline {
        Outline::Polygon { points, .. } => points.iter().flatten().copied().collect(),
        _ => Vec::new(),
    };
    if floats.is_empty() {
        floats = vec![0.0; 2];
    }
    let bytes = float_bytes(&floats);
    let points = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vidcut shape"),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    gpu.queue.write_buffer(&points, 0, &bytes);
    points
}

fn mask_made<'a>(
    gpu: &Gpu,
    (scratch, room): (&[Texture; 2], &'a [Texture; 2]),
    shapes: &[Shaping],
    encoder: &mut wgpu::CommandEncoder,
) -> &'a wgpu::TextureView {
    let mut kept: Option<usize> = None;
    let none = [Figure::NONE];
    for shape in shapes {
        let figures = match shape.figures.is_empty() {
            true => &none[..],
            false => &shape.figures[..],
        };
        let mut from = 1;
        for (i, figure) in figures.iter().enumerate() {
            let points = shape_points(gpu, figure);
            let views = Views {
                cells: Some(&points),
                ..Views::same(&scratch[from].view)
            };
            let drawn = (&gpu.blur.shape, &shape_bytes(shape, figure, i > 0)[..]);
            draw_step(gpu, drawn, views, &scratch[1 - from].view, encoder);
            from = 1 - from;
        }
        for (pipeline, bytes) in soft_steps(&gpu.blur, shape.soft) {
            let views = Views::same(&scratch[from].view);
            draw_step(
                gpu,
                (pipeline, &bytes[..]),
                views,
                &scratch[1 - from].view,
                encoder,
            );
            from = 1 - from;
        }
        let to = kept.map_or(0, |k| 1 - k);
        let start = kept.map_or(&scratch[1 - from].view, |k| &room[k].view);
        let how = [shape.level, flag(shape.invert), flag(kept.is_none()), 0.0];
        let joined = (&gpu.blur.mask_join, &float_bytes(&how)[..]);
        let views = Views::with(&scratch[from].view, start);
        draw_step(gpu, joined, views, &room[to].view, encoder);
        kept = Some(to);
    }
    &room[kept.unwrap_or(0)].view
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

fn vignette_bytes(shading: &Shading, effect: &Working) -> Vec<u8> {
    let [x, y] = shading.first;
    let [r, g, b] = shading.color;
    let mut out = float_bytes(&[x, y, shading.strength, effect.blend]);
    out.extend(float_bytes(&[r, g, b, flag(effect.use_alpha)]));
    for v in [shading.cells[0], shading.cells[1], shading.mode.code(), 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

fn sharpen_bytes(sharpening: &Sharpening, effect: &Working) -> Vec<u8> {
    float_bytes(&[
        effect.blend,
        flag(effect.use_alpha),
        flag(sharpening.colors()),
        0.0,
    ])
}

pub(super) struct Texture {
    pub(super) texture: wgpu::Texture,
    pub(super) view: wgpu::TextureView,
}

impl Texture {
    pub(super) fn new(gpu: &Gpu, size: (u32, u32)) -> Self {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vidcut layer"),
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

type Tiles = Lru<[u64; 8], Texture, GRAIN_TILES>;

fn spare_ready(gpu: &Gpu, spare: &mut Spare, (w, h): (u32, u32), slots: [u32; 2]) {
    for slot in slots {
        let key = (w, h, slot);
        if spare.get(&key).is_none() {
            spare.insert(key, Texture::new(gpu, (w, h)));
        }
    }
}

#[derive(Default)]
struct Worked {
    cells: Option<vignette::Cells>,
    planes: Option<sharpen::Planes>,
    spare: Spare,
    tiles: Tiles,
    shrink: [f64; 2],
    masks: Option<[Texture; 2]>,
}

#[derive(Default)]
pub(super) struct Composer {
    canvas: Option<Texture>,
    scratch: Option<[Texture; 2]>,
    worked: Worked,
}

impl Composer {
    pub(super) fn canvas(&mut self, gpu: &Gpu, size: (u32, u32)) -> &wgpu::TextureView {
        let sized = |c: &Texture| (c.texture.width(), c.texture.height()) == size;
        if !self.canvas.as_ref().is_some_and(sized) {
            *self = Self {
                canvas: Some(Texture::new(gpu, size)),
                ..Self::default()
            };
            self.worked.shrink = [1.0, 1.0];
        }
        &self.canvas.as_ref().expect("made above").view
    }

    pub(super) fn compose(
        &mut self,
        gpu: &Gpu,
        works: &[Working],
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let Some(canvas) = self.canvas.as_ref() else {
            return;
        };
        let size = (canvas.texture.width(), canvas.texture.height());
        let scratch = self
            .scratch
            .get_or_insert_with(|| [Texture::new(gpu, size), Texture::new(gpu, size)]);
        effects_into(gpu, canvas, (scratch, &mut self.worked), works, encoder);
    }
}

#[derive(Default)]
pub(super) struct Blurred {
    size: (u32, u32),
    scratch: Option<[Texture; 2]>,
    layers: Vec<Option<Texture>>,
    masks: Vec<Option<Texture>>,
    worked: Worked,
}

impl Blurred {
    pub(super) fn sized(&mut self, size: (u32, u32), frame: (u32, u32), n: usize) {
        if self.size != size {
            self.size = size;
            self.scratch = None;
            self.worked = Worked::default();
            self.layers.clear();
            self.masks.clear();
        }
        let part = |drawn: u32, whole: u32| (f64::from(drawn) / f64::from(whole.max(1))).min(1.0);
        self.worked.shrink = [part(size.0, frame.0), part(size.1, frame.1)];
        self.layers.resize_with(n, || None);
        self.masks.resize_with(n, || None);
    }

    pub(super) fn make_scratch(&mut self, gpu: &Gpu) {
        let size = self.size;
        self.scratch
            .get_or_insert_with(|| [Texture::new(gpu, size), Texture::new(gpu, size)]);
    }

    pub(super) fn canvas_effects(
        &mut self,
        gpu: &Gpu,
        canvas: &Texture,
        (placing, effects, kernel): (&Placing, &[Working], u32),
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let scratch = self
            .scratch
            .as_ref()
            .expect("made for the effects of a mix");
        let cut = canvas_bind(gpu, canvas, placing, (0.0, 0.0), kernel);
        add(gpu, &scratch[0].view, &cut, encoder);
        encoder.copy_texture_to_texture(
            scratch[0].texture.as_image_copy(),
            canvas.texture.as_image_copy(),
            canvas.texture.size(),
        );
        effects_into(gpu, canvas, (scratch, &mut self.worked), effects, encoder);
    }

    pub(super) fn composed(
        &mut self,
        gpu: &Gpu,
        canvas: &Texture,
        works: &[Working],
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let size = self.size;
        let scratch = self
            .scratch
            .get_or_insert_with(|| [Texture::new(gpu, size), Texture::new(gpu, size)]);
        effects_into(gpu, canvas, (scratch, &mut self.worked), works, encoder);
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
        effects_into(gpu, own, (scratch, &mut self.worked), effects, encoder);
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
        effects_into(gpu, mask, (scratch, &mut self.worked), effects, encoder);
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
        effects_into(gpu, own, (scratch, &mut self.worked), live, encoder);
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
    cells: Option<&'a wgpu::Buffer>,
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
            cells: None,
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
    let mut entries = vec![
        wgpu::BindGroupEntry {
            binding: 0,
            resource: params.as_entire_binding(),
        },
        texture(1, views.source),
        texture(2, views.start),
        texture(14, views.second),
        texture(15, views.mask),
    ];
    entries.extend(views.cells.map(|buffer| wgpu::BindGroupEntry {
        binding: 3,
        resource: buffer.as_entire_binding(),
    }));
    let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("vidcut blur"),
        layout: if views.cells.is_some() {
            &gpu.blur.reading
        } else {
            &gpu.blur.layout
        },
        entries: &entries,
    });
    let mut drawing = begin(encoder, to, None);
    drawing.set_pipeline(pipeline);
    drawing.set_bind_group(0, &bind, &[]);
    drawing.draw(0..3, 0..1);
}

fn blurred_between<'a>(
    gpu: &Gpu,
    (ways, border): (&[(Pass, bool)], u32),
    (first, window): ([f64; 4], [f64; 4]),
    (a, b): (&'a Texture, &'a Texture),
    encoder: &mut wgpu::CommandEncoder,
) -> &'a Texture {
    let (mut from, mut to) = (a, b);
    for (j, (pass, down)) in ways.iter().enumerate() {
        let more = if j == 0 { first } else { [0.0; 4] };
        let bytes = pass_bytes(pass, *down, border, more, window);
        let step = (&gpu.blur.blur, &bytes[..]);
        draw_step(gpu, step, Views::same(&from.view), &to.view, encoder);
        std::mem::swap(&mut from, &mut to);
    }
    from
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
    let ways = grid.ways();
    &blurred_between(gpu, (&ways, border), ([0.0; 4], window), (a, b), encoder).view
}

fn area_bytes(masking: &Masking) -> Vec<u8> {
    let shape = match masking.shape {
        AreaShape::Ellipse => 0.0,
        AreaShape::Box => 1.0,
        AreaShape::Polygon => 2.0,
    };
    let count = masking.maps.len() as f64;
    let points = masking.points.len() as f64;
    float_bytes(&[
        shape,
        masking.softness,
        flag(masking.invert),
        count,
        points,
        0.0,
        0.0,
        0.0,
    ])
}

fn area_maps(gpu: &Gpu, masking: &Masking) -> wgpu::Buffer {
    let maps = masking.maps.iter().flatten().flatten();
    let floats: Vec<f64> = maps
        .chain(masking.points.iter().flatten())
        .copied()
        .collect();
    let bytes = float_bytes(&floats);
    let maps = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vidcut area"),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    gpu.queue.write_buffer(&maps, 0, &bytes);
    maps
}

fn landed(
    gpu: &Gpu,
    own: &Texture,
    (made, free): (&Texture, &Texture),
    (area, mask): (Option<&Masking>, Option<&wgpu::TextureView>),
    encoder: &mut wgpu::CommandEncoder,
) {
    let from = match (area, mask) {
        (_, Some(mask)) => {
            let views = Views {
                mask,
                ..Views::with(&made.view, &own.view)
            };
            draw_step(
                gpu,
                (&gpu.blur.masked, &[0; 16]),
                views,
                &free.view,
                encoder,
            );
            free
        }
        (Some(masking), None) => {
            let maps = area_maps(gpu, masking);
            let step = (&gpu.blur.area, &area_bytes(masking)[..]);
            let views = Views {
                cells: Some(&maps),
                ..Views::with(&made.view, &own.view)
            };
            draw_step(gpu, step, views, &free.view, encoder);
            free
        }
        (None, None) => made,
    };
    encoder.copy_texture_to_texture(
        from.texture.as_image_copy(),
        own.texture.as_image_copy(),
        own.texture.size(),
    );
}

fn glow_into(
    gpu: &Gpu,
    own: &Texture,
    (scratch, spare): (&[Texture; 2], &mut Spare),
    (glow, effect, on): (&Glowing, &Working, Option<&wgpu::TextureView>),
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
        cells: None,
    };
    let [free, made] = scratch;
    draw_step(
        gpu,
        (&gpu.blur.glow, &bytes[..]),
        views,
        &made.view,
        encoder,
    );
    landed(gpu, own, (made, free), (effect.area.as_ref(), on), encoder);
}

fn gaussian_ways(sigma: f64) -> Vec<(Pass, bool)> {
    let want = [sigma, sigma, sigma, 0.0];
    passes(want)
        .into_iter()
        .map(|p| (p, false))
        .chain(passes(want).into_iter().map(|p| (p, true)))
        .collect()
}

fn grain_bytes(grain: &Graining, effect: &Working, (shift, detailed): ([i32; 2], bool)) -> Vec<u8> {
    let [c0, c1, c2, c3] = grain.curve;
    let [low, high, near] = grain.tails;
    let [gr, gg, gb] = grain.gains;
    let [ts, tm, th] = grain.tones;
    let mut out = float_bytes(&[grain.amount, grain.saturation, grain.offset, grain.opacity]);
    out.extend(float_bytes(&[c0, c1, c2, c3]));
    out.extend(float_bytes(&[low, high, near, 0.0]));
    out.extend(float_bytes(&[gr, gg, gb, effect.blend]));
    out.extend(float_bytes(&[ts, tm, th, 0.0]));
    let flags = i32::from(grain.only) * GRAIN_ONLY + i32::from(detailed) * GRAIN_DETAIL;
    for v in [shift[0], shift[1], grain.mode.code() as i32, flags] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

fn grain_tile<'a>(
    gpu: &Gpu,
    tiles: &'a mut Tiles,
    (tile, shrunk): (&GrainTile, &grain::Shrunk),
    (w, h): (u32, u32),
) -> Option<&'a Texture> {
    let key = [
        tile.ring.to_bits(),
        tile.width.to_bits(),
        tile.scale.to_bits(),
        tile.aspect.to_bits(),
        tile.seed,
        shrunk.shrink[0].to_bits(),
        shrunk.shrink[1].to_bits(),
        shrunk.soft.to_bits(),
    ];
    if !tiles.contains(&key) {
        let made = Texture::new(gpu, (w, h));
        gpu.queue.write_texture(
            made.texture.as_image_copy(),
            &grain::tile_bytes(tile, (w, h), shrunk),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * grain::BYTES as u32),
                rows_per_image: Some(h),
            },
            made.texture.size(),
        );
        tiles.insert(key, made);
    }
    tiles.get(&key)
}

fn grain_into(
    gpu: &Gpu,
    own: &Texture,
    (scratch, spare, tiles, shrink): (&[Texture; 2], &mut Spare, &mut Tiles, [f64; 2]),
    (grain, effect, on): (&Graining, &Working, Option<&wgpu::TextureView>),
    encoder: &mut wgpu::CommandEncoder,
) {
    let size = own.texture.size();
    let canvas = (size.width, size.height);
    let whole = shrink.iter().all(|s| *s >= 1.0);
    let shrunk = grain::Shrunk {
        shrink,
        soft: if whole { 0.0 } else { grain.softness },
    };
    let Some(tile) = grain_tile(gpu, tiles, (&grain.tile, &shrunk), canvas) else {
        return;
    };
    let detail = gaussian_ways(grain.detail);
    let mut input = &own.view;
    if !detail.is_empty() {
        spare_ready(gpu, spare, canvas, DETAIL_SLOTS);
        let slots = DETAIL_SLOTS.map(|slot| spare.peek(&(canvas.0, canvas.1, slot)));
        if let [Some(a), Some(b)] = slots {
            encoder.copy_texture_to_texture(
                own.texture.as_image_copy(),
                a.texture.as_image_copy(),
                size,
            );
            let first = [1.0, 0.0, 0.0, 1.0];
            let ways = (&detail[..], BORDER_HALF);
            input = &blurred_between(gpu, ways, (first, [0.0; 4]), (a, b), encoder).view;
        }
    }
    let shift = grain::shift(grain.tile.seed, grain.frame, canvas);
    let bytes = grain_bytes(grain, effect, (shift, !detail.is_empty()));
    let noise = (&gpu.blur.grain_noise, &bytes[..]);
    draw_step(
        gpu,
        noise,
        Views::same(&tile.view),
        &scratch[0].view,
        encoder,
    );
    let soft = gaussian_ways(if whole { grain.softness } else { 0.0 });
    let ways = (&soft[..], BORDER_HALF);
    let plain = ([0.0; 4], [0.0; 4]);
    let [one, two] = scratch;
    let blurred = blurred_between(gpu, ways, plain, (one, two), encoder);
    let (made, free) = if soft.len().is_multiple_of(2) {
        (two, one)
    } else {
        (one, two)
    };
    let views = Views {
        source: &blurred.view,
        start: &own.view,
        second: &own.view,
        mask: input,
        cells: None,
    };
    let last = (&gpu.blur.grain, &bytes[..]);
    draw_step(gpu, last, views, &made.view, encoder);
    landed(gpu, own, (made, free), (effect.area.as_ref(), on), encoder);
}

fn effects_into(
    gpu: &Gpu,
    own: &Texture,
    (scratch, worked): (&[Texture; 2], &mut Worked),
    effects: &[Working],
    encoder: &mut wgpu::CommandEncoder,
) {
    let pipes = &gpu.blur;
    let size = own.texture.size();
    let masked = effects.iter().any(|e| !e.masks.is_empty());
    if masked
        && worked
            .masks
            .as_ref()
            .is_none_or(|m| m[0].texture.size() != size)
    {
        let room = (size.width, size.height);
        worked.masks = Some([Texture::new(gpu, room), Texture::new(gpu, room)]);
    }
    for effect in effects {
        if matches!(effect.work, Work::Clear) {
            drop(begin(encoder, &own.view, Some(wgpu::Color::TRANSPARENT)));
            continue;
        }
        let on = match (&worked.masks, effect.masks.is_empty()) {
            (Some(room), false) => Some(mask_made(gpu, (scratch, room), &effect.masks, encoder)),
            _ => None,
        };
        let mut from = &own.view;
        let mut k = 0;
        let read = match &effect.work {
            Work::Glow(glow) => {
                let room = (scratch, &mut worked.spare);
                glow_into(gpu, own, room, (glow.as_ref(), effect, on), encoder);
                continue;
            }
            Work::Grain(grain) => {
                let room = (scratch, &mut worked.spare, &mut worked.tiles, worked.shrink);
                grain_into(gpu, own, room, (grain.as_ref(), effect, on), encoder);
                continue;
            }
            Work::Vignette(shading) => {
                Some(vignette::cells(gpu, shading, &mut worked.cells, encoder))
            }
            Work::Sharpen(sharpening) => {
                let taken = (&pipes.sharpen_taken, &sharpen_bytes(sharpening, effect)[..]);
                let to = &scratch[0].view;
                draw_step(gpu, taken, Views::with(from, &own.view), to, encoder);
                from = to;
                k = 1;
                let on = (to, own.texture.size());
                Some(sharpen::planes(
                    gpu,
                    sharpening,
                    on,
                    &mut worked.planes,
                    encoder,
                ))
            }
            _ => None,
        };
        for step in steps(pipes, effect) {
            let to = &scratch[k];
            let views = Views {
                cells: read,
                ..Views::with(from, &own.view)
            };
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
        let ends = (&scratch[1 - k], &scratch[k]);
        landed(gpu, own, ends, (effect.area.as_ref(), on), encoder);
    }
}

pub(super) fn opened(placing: &Placing) -> Placing {
    let mut out = placing.clone();
    out.level.open();
    out
}

fn levels_bind(
    gpu: &Gpu,
    view: &wgpu::TextureView,
    placing: &Placing,
    [opacity, mode, adds, made]: [f64; 4],
    (size, kernel): (wgpu::Extent3d, u32),
) -> wgpu::BindGroup {
    let levels = super::level_floats(Some(&placing.level));
    let mut floats = vec![0.0_f64; super::PARAMS / 4 - levels.len()];
    floats[4..6].copy_from_slice(&[f64::from(size.width), f64::from(size.height)]);
    floats[40..45].copy_from_slice(&[1.0, opacity, mode, adds, made]);
    floats.extend(levels);
    let mut bytes = float_bytes(&floats);
    bytes[KERNEL * 4..(KERNEL + 1) * 4].copy_from_slice(&kernel.to_le_bytes());
    bytes_bind(gpu, &bytes, (view, view))
}

fn mask_bind(gpu: &Gpu, view: &wgpu::TextureView, placing: &Placing) -> wgpu::BindGroup {
    levels_bind(
        gpu,
        view,
        placing,
        [1.0, 0.0, 0.0, 4.0],
        (wgpu::Extent3d::default(), 0),
    )
}

pub(super) fn canvas_bind(
    gpu: &Gpu,
    canvas: &Texture,
    placing: &Placing,
    (mode, adds): (f64, f64),
    kernel: u32,
) -> wgpu::BindGroup {
    let made = [1.0, mode, adds, 6.0];
    levels_bind(
        gpu,
        &canvas.view,
        placing,
        made,
        (canvas.texture.size(), kernel),
    )
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
    let levels = super::level_floats(Some(&level));
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
