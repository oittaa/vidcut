#[cfg(windows)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use vidcut_av::color::shader_colors;
use vidcut_av::cuda::{CudaDevice, PlaneCopy};
use vidcut_av::hw::Kind;
use vidcut_av::player::{Layout, Picture, TitleOver, TitlePicture};
use vidcut_av::thumbs::Image;
use vidcut_core::model::Hdr;

pub struct Gpu {
    #[cfg(any(target_os = "linux", windows))]
    acquirer: OnceLock<Result<super::acquire::Acquirer, String>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    blended: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    lettering: wgpu::RenderPipeline,
    lettering_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    wide: bool,
    shares: bool,
    dma_buf: bool,
    d3d11: bool,
    #[cfg(windows)]
    p010: bool,
    #[cfg(windows)]
    d3d11_shared: std::sync::Mutex<Vec<D3d11Shared>>,
    metal: bool,
    cuda: OnceLock<Result<CudaDevice, String>>,
    gate: Option<Arc<super::gate::Gate>>,
}

const MAX_SHARED: usize = 4;

const TARGET: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub fn features(adapter: &wgpu::Adapter) -> wgpu::Features {
    let wanted = wgpu::Features::TEXTURE_FORMAT_16BIT_NORM;
    #[cfg(target_os = "linux")]
    let wanted = wanted | wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF;
    #[cfg(windows)]
    let wanted = wanted
        | wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32
        | wgpu::Features::TEXTURE_FORMAT_NV12
        | wgpu::Features::TEXTURE_FORMAT_P010;
    adapter.features() & wanted
}

impl Gpu {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vidcut picture"),
            source: wgpu::ShaderSource::Wgsl(include_str!("picture.wgsl").into()),
        });
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
            label: Some("vidcut picture"),
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
                texture(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vidcut picture"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline_with = |blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("vidcut picture"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: TARGET,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = pipeline_with(None);
        let over = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Constant,
            dst_factor: wgpu::BlendFactor::OneMinusConstant,
            operation: wgpu::BlendOperation::Add,
        };
        let blended = pipeline_with(Some(wgpu::BlendState {
            color: over,
            alpha: over,
        }));
        let lettering_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vidcut title"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture(6),
            ],
        });
        let lettering_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("vidcut title"),
                bind_group_layouts: &[Some(&lettering_layout)],
                immediate_size: 0,
            });
        let premultiplied = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let lettering = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vidcut title"),
            layout: Some(&lettering_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_title"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TARGET,
                    blend: Some(wgpu::BlendState {
                        color: premultiplied,
                        alpha: premultiplied,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("vidcut picture"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            #[cfg(any(target_os = "linux", windows))]
            acquirer: OnceLock::new(),
            device: device.clone(),
            queue: queue.clone(),
            pipeline,
            blended,
            layout,
            lettering,
            lettering_layout,
            sampler,
            wide: device
                .features()
                .contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM),
            shares: super::shared::possible(device),
            dma_buf: device
                .features()
                .contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF),
            d3d11: device.features().contains(
                wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32 | wgpu::Features::TEXTURE_FORMAT_NV12,
            ),
            #[cfg(windows)]
            p010: device
                .features()
                .contains(wgpu::Features::TEXTURE_FORMAT_P010),
            #[cfg(windows)]
            d3d11_shared: std::sync::Mutex::default(),
            // SAFETY: wgpu's own device, only asked what it is.
            #[cfg(target_os = "macos")]
            metal: unsafe { device.as_hal::<wgpu::hal::api::Metal>() }.is_some(),
            #[cfg(not(target_os = "macos"))]
            metal: false,
            cuda: OnceLock::new(),
            gate: None,
        }
    }

    pub(super) fn behind(mut self, gate: Arc<super::gate::Gate>) -> Self {
        self.gate = Some(gate);
        self
    }

    fn submit(
        &self,
        buffers: impl IntoIterator<Item = wgpu::CommandBuffer>,
    ) -> wgpu::SubmissionIndex {
        match &self.gate {
            Some(gate) => gate.pass(|| self.queue.submit(buffers)),
            None => self.queue.submit(buffers),
        }
    }

    pub fn gpu_pictures(&self) -> Vec<Kind> {
        let cudas = vidcut_av::gpus::in_use().and_then(|g| g.uuid());
        shown_from_gpu(&Takes {
            shares: self.shares,
            wide: self.wide,
            on_cudas_gpu: cudas.is_some() && cudas == super::shared::uuid(&self.device),
            dma_buf: self.dma_buf,
            d3d11: self.d3d11,
            metal: self.metal,
        })
    }

    #[cfg(any(target_os = "linux", windows))]
    unsafe fn acquire(
        &self,
        images: &[ash::vk::Image],
        layout: ash::vk::ImageLayout,
        stage: ash::vk::PipelineStageFlags,
        access: ash::vk::AccessFlags,
        source: super::acquire::Source,
    ) -> Result<wgpu::CommandBuffer, String> {
        // SAFETY: as the caller says.
        unsafe {
            self.barrier(|acquirer, cmd| {
                acquirer.record(cmd, images, source, layout, stage, access)
            })
        }
    }

    #[cfg(windows)]
    unsafe fn release(
        &self,
        images: &[ash::vk::Image],
        layout: (ash::vk::ImageLayout, ash::vk::ImageLayout),
        stage: ash::vk::PipelineStageFlags,
        access: ash::vk::AccessFlags,
    ) -> Result<wgpu::CommandBuffer, String> {
        // SAFETY: as the caller says.
        unsafe {
            self.barrier(|acquirer, cmd| acquirer.release(cmd, images, layout, stage, access))
        }
    }

    #[cfg(any(target_os = "linux", windows))]
    unsafe fn barrier(
        &self,
        record: impl FnOnce(&super::acquire::Acquirer, ash::vk::CommandBuffer),
    ) -> Result<wgpu::CommandBuffer, String> {
        let acquirer = self
            .acquirer
            .get_or_init(|| {
                // SAFETY: wgpu's own device, only read here.
                let hal = unsafe { self.device.as_hal::<wgpu::hal::api::Vulkan>() };
                hal.map(|hal| super::acquire::Acquirer::new(&hal))
                    .ok_or_else(|| "not Vulkan".to_string())
            })
            .as_ref()?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("vidcut acquire"),
            });
        // SAFETY: this encoder's open buffer gets only the barrier; images as the caller says.
        let recorded = unsafe {
            encoder.as_hal_mut::<wgpu::hal::api::Vulkan, _, _>(|hal| {
                hal.map(|hal| record(acquirer, hal.raw_handle()))
            })
        };
        recorded.ok_or("not Vulkan")?;
        Ok(encoder.finish())
    }

    fn checked<T>(&self, f: impl FnOnce() -> T) -> Result<T, String> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let made = f();
        match pollster::block_on(scope.pop()) {
            Some(e) => Err(format!("the display's device refused it: {e}")),
            None => Ok(made),
        }
    }

    fn cuda(&self) -> Result<&CudaDevice, String> {
        self.cuda
            .get_or_init(|| super::shared::cuda_on(&self.device))
            .as_ref()
            .map_err(Clone::clone)
    }
}

#[derive(Debug, Default)]
struct Takes {
    shares: bool,
    wide: bool,
    on_cudas_gpu: bool,
    dma_buf: bool,
    d3d11: bool,
    metal: bool,
}

fn shown_from_gpu(takes: &Takes) -> Vec<Kind> {
    [
        (Kind::Nvdec, takes.shares),
        (
            Kind::Vulkan,
            takes.shares && takes.wide && takes.on_cudas_gpu,
        ),
        (Kind::Vaapi, takes.dma_buf),
        (Kind::D3d11va, takes.d3d11),
        (Kind::VideoToolbox, takes.metal),
    ]
    .into_iter()
    .filter_map(|(kind, shown)| shown.then_some(kind))
    .collect()
}

struct Planes {
    key: (Layout, u32, u32, bool),
    y: wgpu::Texture,
    u: wgpu::Texture,
    v: wgpu::Texture,
}

struct Target {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
}

#[derive(Default)]
pub struct Screen {
    target: Option<Target>,
    drawn: Option<Option<Arc<Picture>>>,
    drawn_titles: Vec<TitleOver>,
    slots: Vec<Slot>,
    letters: Vec<Letters>,
    imports_before: u64,
}

struct Letters {
    picture: Arc<TitlePicture>,
    params: wgpu::Buffer,
    bind: wgpu::BindGroup,
}

impl Letters {
    fn new(gpu: &Gpu, picture: &Arc<TitlePicture>) -> Self {
        let size = wgpu::Extent3d {
            width: picture.width.max(1),
            height: picture.height.max(1),
            depth_or_array_layers: 1,
        };
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vidcut title"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let row = picture.width as usize * 4;
        if row > 0 && picture.rgba.len() >= row * picture.height as usize {
            gpu.queue.write_texture(
                texture.as_image_copy(),
                &picture.rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row as u32),
                    rows_per_image: Some(picture.height),
                },
                size,
            );
        }
        let params = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vidcut title"),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view = texture.create_view(&Default::default());
        let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vidcut title"),
            layout: &gpu.lettering_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&gpu.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        Self {
            picture: picture.clone(),
            params,
            bind,
        }
    }

    fn place(&self, gpu: &Gpu, size: (u32, u32), t: &TitleOver) {
        let (dx, dy, alpha) = (t.dx, t.dy, t.alpha);
        let p = &self.picture;
        let (fw, fh) = (f64::from(p.frame.0.max(1)), f64::from(p.frame.1.max(1)));
        let (sx, sy) = (f64::from(size.0) / fw, f64::from(size.1) / fh);
        let place = [
            f64::from(p.left + dx) * sx,
            f64::from(p.top + dy) * sy,
            f64::from(p.width) * sx,
            f64::from(p.height) * sy,
            f64::from(alpha),
            0.0,
            0.0,
            0.0,
            f64::from(t.turn[0]) * sx,
            f64::from(t.turn[1]) * sy,
            f64::from(t.turn[2]),
            f64::from(t.turn[3]),
        ];
        let bytes: Vec<u8> = place
            .iter()
            .flat_map(|v| (*v as f32).to_le_bytes())
            .collect();
        gpu.queue.write_buffer(&self.params, 0, &bytes);
    }
}

fn same_titles(a: &[TitleOver], b: &[TitleOver]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.above == y.above
                && x.dx == y.dx
                && x.dy == y.dy
                && x.turn == y.turn
                && x.alpha == y.alpha
                && Arc::ptr_eq(&x.picture, &y.picture)
        })
}

#[derive(Default)]
struct Slot {
    planes: Option<Planes>,
    params: Option<wgpu::Buffer>,
    shared: Vec<super::shared::Shared>,
    filled: Option<usize>,
    unshared: bool,
    unimported: Vec<Layout>,
    imported: Option<Arc<Picture>>,
    views: Option<(wgpu::TextureView, wgpu::TextureView, (u32, u32))>,
    v_view: Option<wgpu::TextureView>,
    acquired: Option<wgpu::CommandBuffer>,
    #[cfg(any(target_os = "linux", windows))]
    kept: super::vulkan::Imports,
    released: Option<wgpu::CommandBuffer>,
    #[cfg(windows)]
    d3d11_used: Option<Arc<AtomicBool>>,
    imports: u64,
    last_shared: bool,
}

#[cfg(windows)]
struct D3d11Shared {
    texture: vidcut_av::hw::d3d11::Shared,
    image: ash::vk::Image,
    y: wgpu::TextureView,
    uv: wgpu::TextureView,
    size: (u32, u32),
    busy: Arc<AtomicBool>,
    shown: std::time::Instant,
}

#[cfg(windows)]
type D3d11Formats = (
    wgpu::TextureFormat,
    wgpu::TextureFormat,
    wgpu::TextureFormat,
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drawn {
    Same,
    Picture,
    Target,
}

impl Screen {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn view(&self) -> Option<&wgpu::TextureView> {
        self.target.as_ref().map(|t| &t.view)
    }

    pub fn shared(&self) -> bool {
        self.slots.first().is_some_and(|s| s.last_shared)
    }

    pub fn imports(&self) -> u64 {
        self.imports_before + self.slots.iter().map(|s| s.imports).sum::<u64>()
    }

    pub fn draw(
        &mut self,
        gpu: &Gpu,
        picture: Option<&Arc<Picture>>,
        titles: &[TitleOver],
        width: u32,
        height: u32,
    ) -> Drawn {
        let max = gpu.device.limits().max_texture_dimension_2d;
        let size = (width.clamp(2, max), height.clamp(2, max));
        let mut drawn = Drawn::Picture;
        if self.target.as_ref().is_none_or(|t| t.size != size) {
            self.target = Some(make_target(&gpu.device, size));
            drawn = Drawn::Target;
        } else {
            let same = match (&self.drawn, picture) {
                (Some(Some(a)), Some(b)) => Arc::ptr_eq(a, b),
                (Some(None), None) => true,
                _ => false,
            } && same_titles(&self.drawn_titles, titles);
            if same {
                return Drawn::Same;
            }
        }
        let limit = gpu.device.limits().max_texture_dimension_2d;
        let fits = |p: &Arc<Picture>| p.width() <= limit && p.height() <= limit;
        let picture = picture.filter(|p| fits(p));
        let layers: Vec<(&Arc<Picture>, Option<f64>)> = picture
            .into_iter()
            .map(|p| (p, None))
            .chain(picture.into_iter().flat_map(|p| {
                p.over()
                    .iter()
                    .filter(|o| fits(&o.picture))
                    .map(|o| (&o.picture, Some(f64::from(o.level()))))
            }))
            .collect();
        if self.slots.len() < layers.len() {
            self.slots.resize_with(layers.len(), Slot::default);
        }
        #[cfg(any(target_os = "linux", windows))]
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if layers
                .get(i)
                .is_none_or(|(p, _)| p.gpu() != Some(Kind::Vulkan))
            {
                slot.kept.clear();
            }
        }
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("vidcut picture"),
            });
        let binds: Vec<(wgpu::BindGroup, Option<f64>)> = layers
            .iter()
            .zip(&mut self.slots)
            .map(|(&(picture, level), slot)| {
                (slot.prepare(gpu, picture, size, &mut encoder), level)
            })
            .collect();
        self.letters
            .retain(|l| titles.iter().any(|t| Arc::ptr_eq(&t.picture, &l.picture)));
        for t in titles {
            if !self
                .letters
                .iter()
                .any(|l| Arc::ptr_eq(&l.picture, &t.picture))
            {
                self.letters.push(Letters::new(gpu, &t.picture));
            }
        }
        for l in &self.letters {
            if let Some(t) = titles.iter().find(|t| Arc::ptr_eq(&t.picture, &l.picture)) {
                l.place(gpu, size, t);
            }
        }
        let target = self.target.as_ref().expect("made above");
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("vidcut picture"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let drawn_layers = binds.len();
            for (i, (bind, level)) in binds.iter().enumerate() {
                match level {
                    None => pass.set_pipeline(&gpu.pipeline),
                    Some(l) => {
                        pass.set_pipeline(&gpu.blended);
                        pass.set_blend_constant(wgpu::Color {
                            r: *l,
                            g: *l,
                            b: *l,
                            a: *l,
                        });
                    }
                }
                pass.set_bind_group(0, bind, &[]);
                pass.draw(0..3, 0..1);
                let after = i + 1;
                for t in titles
                    .iter()
                    .filter(|t| t.above == after || (after == drawn_layers && t.above > after))
                {
                    let Some(l) = self
                        .letters
                        .iter()
                        .find(|l| Arc::ptr_eq(&l.picture, &t.picture))
                    else {
                        continue;
                    };
                    pass.set_pipeline(&gpu.lettering);
                    pass.set_bind_group(0, &l.bind, &[]);
                    pass.draw(0..3, 0..1);
                }
            }
        }
        let acquired: Vec<wgpu::CommandBuffer> = self.slots[..layers.len()]
            .iter_mut()
            .filter_map(|s| s.acquired.take())
            .collect();
        let released: Vec<wgpu::CommandBuffer> = self.slots[..layers.len()]
            .iter_mut()
            .filter_map(|s| s.released.take())
            .collect();
        gpu.submit(
            acquired
                .into_iter()
                .chain([encoder.finish()])
                .chain(released),
        );
        for slot in &mut self.slots[..layers.len()] {
            slot.submitted(gpu);
        }
        let keep = layers.len().max(1);
        self.imports_before += self.slots.iter().skip(keep).map(|s| s.imports).sum::<u64>();
        self.slots.truncate(keep);
        self.drawn = Some(picture.cloned());
        self.drawn_titles = titles.to_vec();
        drawn
    }
}

impl Slot {
    fn prepare(
        &mut self,
        gpu: &Gpu,
        picture: &Arc<Picture>,
        size: (u32, u32),
        encoder: &mut wgpu::CommandEncoder,
    ) -> wgpu::BindGroup {
        self.filled = None;
        self.upload(gpu, picture, encoder);
        let params = self.params.get_or_insert_with(|| {
            gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("vidcut picture"),
                size: PARAMS as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        let cover = self.views.as_ref().map_or((1.0, 1.0), |&(_, _, (w, h))| {
            (
                f64::from(picture.width()) / f64::from(w),
                f64::from(picture.height()) / f64::from(h),
            )
        });
        gpu.queue
            .write_buffer(params, 0, &params_of(picture, size, gpu.wide, cover));
        let planes = self.planes.as_ref().expect("uploaded");
        let view = |t: &wgpu::Texture| t.create_view(&Default::default());
        let (y, u) = self
            .views
            .take()
            .map(|(y, u, _)| (y, u))
            .unwrap_or_else(|| (view(&planes.y), view(&planes.u)));
        let v = self.v_view.take().unwrap_or_else(|| view(&planes.v));
        gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vidcut picture"),
            layout: &gpu.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&y),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&u),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&v),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&gpu.sampler),
                },
            ],
        })
    }

    fn submitted(&mut self, gpu: &Gpu) {
        if let Some(i) = self.filled.take() {
            self.shared[i].used(&gpu.queue);
        }
        #[cfg(windows)]
        if let Some(busy) = self.d3d11_used.take() {
            gpu.queue
                .on_submitted_work_done(move || busy.store(false, Ordering::Release));
        }
        if let Some(held) = self.imported.take() {
            gpu.queue.on_submitted_work_done(move || drop(held));
        }
    }

    fn upload(&mut self, gpu: &Gpu, picture: &Arc<Picture>, encoder: &mut wgpu::CommandEncoder) {
        self.make_planes(gpu, picture);
        self.last_shared = false;
        match picture.gpu() {
            None => return self.write_planes(gpu, picture),
            Some(Kind::Nvdec) => {}
            Some(kind) => {
                let layout = picture.layout();
                if !self.unimported.contains(&layout) {
                    let imported = match kind {
                        #[cfg(target_os = "linux")]
                        Kind::Vaapi => self.import_on_gpu(gpu, picture, encoder),
                        #[cfg(windows)]
                        Kind::D3d11va => self.import_d3d11(gpu, picture),
                        #[cfg(target_os = "macos")]
                        Kind::VideoToolbox => self.import_videotoolbox(gpu, picture),
                        #[cfg(any(target_os = "linux", windows))]
                        Kind::Vulkan => self.import_vulkan(gpu, picture),
                        _ => Err(format!("{} pictures aren't this system's", kind.label())),
                    };
                    match imported {
                        Ok(()) => {
                            self.last_shared = true;
                            return;
                        }
                        Err(e) => {
                            eprintln!(
                                "vidcut: {layout:?} pictures come down from the GPU to be shown: {e}"
                            );
                            self.unimported.push(layout);
                        }
                    }
                }
                return self.copy_down(gpu, picture);
            }
        }
        if !self.unshared {
            match self.copy_on_gpu(gpu, picture, encoder) {
                Ok(true) => {
                    self.last_shared = true;
                    return;
                }
                Ok(false) => {}
                Err(e) => {
                    eprintln!("vidcut: pictures come down from the GPU to be shown: {e}");
                    self.unshared = true;
                    self.shared.clear();
                }
            }
        }
        self.copy_down(gpu, picture);
    }

    fn copy_down(&mut self, gpu: &Gpu, picture: &Picture) {
        match picture.copied_down() {
            Ok(down) => self.write_planes(gpu, &down),
            Err(e) => eprintln!("vidcut: a picture didn't come down from the GPU: {e}"),
        }
    }

    #[cfg(target_os = "linux")]
    fn import_on_gpu(
        &mut self,
        gpu: &Gpu,
        picture: &Arc<Picture>,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<(), String> {
        use wgpu::hal::api::Vulkan;
        if !gpu.dma_buf {
            return Err("the display's device imports no DMA-BUFs".into());
        }
        let (one, two) = match picture.layout() {
            Layout::Nv12 => (wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rg8Unorm),
            Layout::P010 if gpu.wide => (
                wgpu::TextureFormat::R16Unorm,
                wgpu::TextureFormat::Rg16Unorm,
            ),
            Layout::P010 => return Err("no 16-bit textures for P010".into()),
            layout @ (Layout::P210 | Layout::P416) => {
                return Err(format!("{layout:?} pictures aren't VA-API's"));
            }
            Layout::Yuv420p
            | Layout::Yuv420p10
            | Layout::Yuv422p
            | Layout::Yuv422p10
            | Layout::Yuv444p12 => {
                return Err("planar pictures aren't VA-API's".into());
            }
        };
        let drm =
            vidcut_av::hw::vaapi::DrmFrame::export(picture.frame()).map_err(|e| e.to_string())?;
        let planes = drm.planes();
        let [y, uv, ..] = planes[..] else {
            return Err(format!("a surface exported in {} planes", planes.len()));
        };
        let (w, h) = (picture.width(), picture.height());
        let ours = self.planes.as_ref().expect("made by upload");
        let label = Some("vidcut VA-API plane");
        let mut imports = Vec::new();
        for (plane, (width, height), format, texture) in [
            (y, (w, h), one, &ours.y),
            (uv, (w.div_ceil(2), h.div_ceil(2)), two, &ours.u),
        ] {
            let size = wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            };
            let fd = plane
                .fd
                .try_clone_to_owned()
                .map_err(|e| format!("a surface's file: {e}"))?;
            // SAFETY: `fd` is the plane's DMA-BUF, at its modifier, pitch and offset, as described.
            let raw = unsafe {
                let hal = gpu.device.as_hal::<Vulkan>().ok_or("not Vulkan")?;
                hal.texture_from_dmabuf_fd(
                    fd,
                    &wgpu::hal::TextureDescriptor {
                        label,
                        size,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUses::COPY_SRC,
                        memory_flags: wgpu::hal::MemoryFlags::empty(),
                        view_formats: Vec::new(),
                    },
                    plane.modifier,
                    plane.pitch,
                    plane.offset,
                )
                .map_err(|e| format!("importing a VA-API surface: {e}"))?
            };
            imports.push((raw, size, format, texture));
        }
        let textures = gpu.checked(|| {
            imports
                .into_iter()
                .map(|(raw, size, format, texture)| {
                    // SAFETY: `raw` is this device's, as described; acquired below into `COPY_SRC`.
                    let imported = unsafe {
                        gpu.device.create_texture_from_hal::<Vulkan>(
                            raw,
                            &wgpu::TextureDescriptor {
                                label,
                                size,
                                mip_level_count: 1,
                                sample_count: 1,
                                dimension: wgpu::TextureDimension::D2,
                                format,
                                usage: wgpu::TextureUsages::COPY_SRC,
                                view_formats: &[],
                            },
                            wgpu::TextureUses::COPY_SRC,
                        )
                    };
                    (imported, size, texture)
                })
                .collect::<Vec<_>>()
        })?;
        // SAFETY: this device's images on memory VA-API wrote, which wgpu hasn't used yet.
        unsafe {
            let images: Vec<_> = textures
                .iter()
                .filter_map(|(imported, ..)| imported.as_hal::<Vulkan>().map(|t| t.raw_handle()))
                .collect();
            if images.len() != textures.len() {
                return Err("not Vulkan".into());
            }
            self.acquired = Some(gpu.acquire(
                &images,
                ash::vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                ash::vk::PipelineStageFlags::TRANSFER,
                ash::vk::AccessFlags::TRANSFER_READ,
                super::acquire::Source::Api,
            )?);
        }
        for (imported, size, texture) in textures {
            encoder.copy_texture_to_texture(
                imported.as_image_copy(),
                texture.as_image_copy(),
                size,
            );
        }
        self.imported = Some(picture.clone());
        self.imports += 1;
        Ok(())
    }

    #[cfg(windows)]
    fn import_d3d11(&mut self, gpu: &Gpu, picture: &Arc<Picture>) -> Result<(), String> {
        use ash::vk::{AccessFlags, ImageLayout, PipelineStageFlags};
        if !gpu.d3d11 {
            return Err("the display's device imports no Direct3D 11 textures".into());
        }
        let formats = match picture.layout() {
            Layout::Nv12 => (
                wgpu::TextureFormat::NV12,
                wgpu::TextureFormat::R8Unorm,
                wgpu::TextureFormat::Rg8Unorm,
            ),
            Layout::P010 if gpu.wide && gpu.p010 => (
                wgpu::TextureFormat::P010,
                wgpu::TextureFormat::R16Unorm,
                wgpu::TextureFormat::Rg16Unorm,
            ),
            Layout::P010 => return Err("no P010 or 16-bit textures for P010".into()),
            layout @ (Layout::P210 | Layout::P416) => {
                return Err(format!("{layout:?} pictures aren't Direct3D 11's"));
            }
            Layout::Yuv420p
            | Layout::Yuv420p10
            | Layout::Yuv422p
            | Layout::Yuv422p10
            | Layout::Yuv444p12 => {
                return Err("planar pictures aren't Direct3D 11's".into());
            }
        };
        let frame = picture.frame();
        let mut pool = gpu.d3d11_shared.lock().unwrap_or_else(|e| e.into_inner());
        let (i, new) = d3d11_shared_for(gpu, &mut pool, frame, formats)?;
        if new {
            self.imports += 1;
        }
        let shared = &mut pool[i];
        shared.texture.fill(frame).map_err(|e| e.to_string())?;
        // SAFETY: this device's image, released to Direct3D 11 in `GENERAL`, which has just written it.
        let (acquired, released) = unsafe {
            (
                gpu.acquire(
                    &[shared.image],
                    ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    PipelineStageFlags::FRAGMENT_SHADER,
                    AccessFlags::SHADER_READ,
                    super::acquire::Source::Api,
                )?,
                gpu.release(
                    &[shared.image],
                    (ImageLayout::SHADER_READ_ONLY_OPTIMAL, ImageLayout::GENERAL),
                    PipelineStageFlags::FRAGMENT_SHADER,
                    AccessFlags::SHADER_READ,
                )?,
            )
        };
        shared.shown = std::time::Instant::now();
        shared.busy.store(true, Ordering::Release);
        self.acquired = Some(acquired);
        self.released = Some(released);
        self.views = Some((shared.y.clone(), shared.uv.clone(), shared.size));
        self.d3d11_used = Some(shared.busy.clone());
        Ok(())
    }

    #[cfg(any(target_os = "linux", windows))]
    fn import_vulkan(&mut self, gpu: &Gpu, picture: &Arc<Picture>) -> Result<(), String> {
        if !gpu.shares {
            return Err("the display's device imports no Vulkan memory".into());
        }
        match picture.layout() {
            Layout::Yuv420p10 | Layout::Yuv422p10 | Layout::Yuv444p12 if gpu.wide => {}
            Layout::Yuv420p10 | Layout::Yuv422p10 | Layout::Yuv444p12 => {
                return Err("no 16-bit textures for 10- and 12-bit planes".into());
            }
            layout @ (Layout::Nv12
            | Layout::P010
            | Layout::P210
            | Layout::P416
            | Layout::Yuv420p
            | Layout::Yuv422p) => return Err(format!("{layout:?} pictures aren't Vulkan's")),
        }
        let frame = picture.frame();
        let identity = vidcut_av::hw::vulkan::Identity::of(frame).map_err(|e| e.to_string())?;
        let (import, new) = self
            .kept
            .get_or_import(identity, || import_vulkan_frame(gpu, frame))?;
        let left_in = vidcut_av::hw::vulkan::layout(frame).map_err(|e| e.to_string())?;
        // SAFETY: this device's images on FFmpeg's memory; wgpu left any it used shader-readable.
        let acquired = unsafe {
            gpu.acquire(
                &import.images,
                ash::vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                ash::vk::PipelineStageFlags::FRAGMENT_SHADER,
                ash::vk::AccessFlags::SHADER_READ,
                super::acquire::Source::Vulkan {
                    concurrent: import.concurrent,
                    layout: left_in,
                },
            )?
        };
        vidcut_av::hw::vulkan::acquired_outside(
            frame,
            ash::vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        )
        .map_err(|e| e.to_string())?;
        let [y, u, v] = &import.views;
        self.views = Some((y.clone(), u.clone(), import.size));
        self.v_view = Some(v.clone());
        self.acquired = Some(acquired);
        self.imported = Some(picture.clone());
        if new {
            self.imports += 1;
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn import_videotoolbox(&mut self, gpu: &Gpu, picture: &Arc<Picture>) -> Result<(), String> {
        use objc2_io_surface::IOSurfaceRef;
        use objc2_metal::{
            MTLDevice, MTLPixelFormat, MTLTextureDescriptor, MTLTextureType, MTLTextureUsage,
        };
        use wgpu::TextureFormat as F;
        use wgpu::hal::api::Metal;
        #[link(name = "CoreVideo", kind = "framework")]
        unsafe extern "C" {
            fn CVPixelBufferGetIOSurface(buffer: *mut std::ffi::c_void) -> *const IOSurfaceRef;
        }
        if !gpu.metal {
            return Err("the display's device isn't Metal".into());
        }
        let formats = match picture.layout() {
            Layout::Nv12 => [
                (MTLPixelFormat::R8Unorm, F::R8Unorm),
                (MTLPixelFormat::RG8Unorm, F::Rg8Unorm),
            ],
            Layout::P010 | Layout::P210 | Layout::P416 if gpu.wide => [
                (MTLPixelFormat::R16Unorm, F::R16Unorm),
                (MTLPixelFormat::RG16Unorm, F::Rg16Unorm),
            ],
            layout @ (Layout::P010 | Layout::P210 | Layout::P416) => {
                return Err(format!("no 16-bit textures for {layout:?}"));
            }
            Layout::Yuv420p
            | Layout::Yuv420p10
            | Layout::Yuv422p
            | Layout::Yuv422p10
            | Layout::Yuv444p12 => {
                return Err("planar pictures aren't VideoToolbox's".into());
            }
        };
        // SAFETY: a VideoToolbox frame's `data[3]` is its pixel buffer, alive as long as the frame.
        let surface = unsafe {
            let buffer = (*picture.frame().as_ptr()).data[3].cast();
            CVPixelBufferGetIOSurface(buffer).as_ref()
        }
        .ok_or("a pixel buffer without an IOSurface")?;
        if surface.plane_count() != 2 {
            return Err(format!("an IOSurface of {} planes", surface.plane_count()));
        }
        // SAFETY: wgpu's own device, which makes textures here only.
        let hal = unsafe { gpu.device.as_hal::<Metal>() }.ok_or("not Metal")?;
        let label = Some("vidcut VideoToolbox picture");
        let plane = |index: usize| -> Result<(wgpu::TextureView, (u32, u32)), String> {
            let (mtl, format) = formats[index];
            let (w, h) = (
                surface.width_of_plane(index),
                surface.height_of_plane(index),
            );
            // SAFETY: `desc` is the plane's size and format; Metal checks it against `surface`.
            let raw = unsafe {
                let desc =
                    MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                        mtl, w, h, false,
                    );
                desc.setUsage(MTLTextureUsage::ShaderRead);
                hal.raw_device()
                    .newTextureWithDescriptor_iosurface_plane(&desc, surface, index)
            }
            .ok_or("Metal won't make a texture of the IOSurface")?;
            let (width, height) = (w as u32, h as u32);
            let extent = wgpu::hal::CopyExtent {
                width,
                height,
                depth: 1,
            };
            // SAFETY: `raw` is a texture of this device, as described, which only shaders read.
            let texture = unsafe {
                let raw = wgpu::hal::metal::Device::texture_from_raw(
                    raw,
                    format,
                    MTLTextureType::Type2D,
                    1,
                    1,
                    extent,
                    None,
                );
                gpu.device.create_texture_from_hal::<Metal>(
                    raw,
                    &wgpu::TextureDescriptor {
                        label,
                        size: wgpu::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    },
                    wgpu::TextureUses::RESOURCE,
                )
            };
            Ok((texture.create_view(&Default::default()), (width, height)))
        };
        let (y, uv) = gpu.checked(|| (plane(0), plane(1)))?;
        let ((y, size), (uv, _)) = (y?, uv?);
        self.views = Some((y, uv, size));
        self.imported = Some(picture.clone());
        self.imports += 1;
        Ok(())
    }

    fn copy_on_gpu(
        &mut self,
        gpu: &Gpu,
        picture: &Picture,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<bool, String> {
        let cuda = gpu.cuda()?;
        let layout = picture.layout();
        let bytes = match layout {
            Layout::Nv12 => 1,
            Layout::P010 if gpu.wide => 2,
            Layout::P010 => return Err("no 16-bit textures for P010".into()),
            Layout::P210 => return Ok(false),
            Layout::P416 => return Err("P416 pictures aren't NVDEC's".into()),
            Layout::Yuv420p
            | Layout::Yuv420p10
            | Layout::Yuv422p
            | Layout::Yuv422p10
            | Layout::Yuv444p12 => {
                return Err("planar pictures aren't NVDEC's".into());
            }
        };
        let (w, h) = (u64::from(picture.width()), u64::from(picture.height()));
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let align = |n: u64| n.div_ceil(256) * 256;
        let y = PlaneCopy {
            offset: 0,
            pitch: align(w * bytes),
            row_bytes: w * bytes,
            rows: h,
        };
        let uv = PlaneCopy {
            offset: align(y.pitch * h),
            pitch: align(cw * 2 * bytes),
            row_bytes: cw * 2 * bytes,
            rows: ch,
        };
        let need = uv.offset + uv.pitch * ch;
        self.shared.retain(|s| s.size() >= need);
        let free = |shared: &[super::shared::Shared]| shared.iter().position(|s| s.free());
        let i = match free(&self.shared) {
            Some(i) => i,
            None if self.shared.len() < MAX_SHARED => {
                let made = super::shared::Shared::new(&gpu.device, cuda, need)?;
                self.shared.push(made);
                self.shared.len() - 1
            }
            None => {
                let _ = gpu.device.poll(wgpu::PollType::Poll);
                match free(&self.shared) {
                    Some(i) => i,
                    None => return Ok(false),
                }
            }
        };
        let shared = &mut self.shared[i];
        shared.fill(picture.frame(), &[y, uv])?;
        self.filled = Some(i);
        let planes = self.planes.as_ref().expect("made by upload");
        for (texture, p, width) in [(&planes.y, y, w), (&planes.u, uv, cw)] {
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: shared.buffer(),
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: p.offset,
                        bytes_per_row: Some(p.pitch as u32),
                        rows_per_image: Some(p.rows as u32),
                    },
                },
                texture.as_image_copy(),
                wgpu::Extent3d {
                    width: width as u32,
                    height: p.rows as u32,
                    depth_or_array_layers: 1,
                },
            );
        }
        Ok(true)
    }

    fn make_planes(&mut self, gpu: &Gpu, picture: &Picture) {
        let (w, h) = (picture.width(), picture.height());
        let layout = picture.layout();
        let (cw, ch) = chroma_size(layout, w, h);
        let wide = gpu.wide && sixteen(layout);
        let key = (layout, w, h, wide);
        if self.planes.as_ref().is_none_or(|p| p.key != key) {
            let (one, two) = if wide {
                (
                    wgpu::TextureFormat::R16Unorm,
                    wgpu::TextureFormat::Rg16Unorm,
                )
            } else {
                (wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rg8Unorm)
            };
            let make = |size: (u32, u32), format| {
                gpu.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("vidcut plane"),
                    size: wgpu::Extent3d {
                        width: size.0,
                        height: size.1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
            };
            self.planes = Some(match layout {
                Layout::Yuv420p
                | Layout::Yuv420p10
                | Layout::Yuv422p
                | Layout::Yuv422p10
                | Layout::Yuv444p12 => Planes {
                    key,
                    y: make((w, h), one),
                    u: make((cw, ch), one),
                    v: make((cw, ch), one),
                },
                Layout::Nv12 | Layout::P010 | Layout::P210 | Layout::P416 => Planes {
                    key,
                    y: make((w, h), one),
                    u: make((cw, ch), two),
                    v: make((1, 1), one),
                },
            });
        }
    }

    fn write_planes(&self, gpu: &Gpu, picture: &Picture) {
        let (w, h) = (picture.width(), picture.height());
        let layout = picture.layout();
        let (cw, ch) = chroma_size(layout, w, h);
        let wide = gpu.wide && sixteen(layout);
        let planes = self.planes.as_ref().expect("made for it");
        let frame = picture.frame();
        vidcut_av::copies::up(frame);
        let put = |texture: &wgpu::Texture, plane: usize, size: (u32, u32), bytes: u32| {
            let stride = frame.stride(plane);
            let data = frame.data(plane);
            write_plane(&gpu.queue, texture, data, stride, size, bytes);
        };
        match (layout, wide) {
            (Layout::Yuv420p | Layout::Yuv422p, _) => {
                put(&planes.y, 0, (w, h), 1);
                put(&planes.u, 1, (cw, ch), 1);
                put(&planes.v, 2, (cw, ch), 1);
            }
            (Layout::Yuv420p10 | Layout::Yuv422p10 | Layout::Yuv444p12, true) => {
                put(&planes.y, 0, (w, h), 2);
                put(&planes.u, 1, (cw, ch), 2);
                put(&planes.v, 2, (cw, ch), 2);
            }
            (Layout::Yuv420p10 | Layout::Yuv422p10 | Layout::Yuv444p12, false) => {
                let shift = if layout == Layout::Yuv444p12 { 4 } else { 2 };
                for (texture, plane, size) in [
                    (&planes.y, 0, (w, h)),
                    (&planes.u, 1, (cw, ch)),
                    (&planes.v, 2, (cw, ch)),
                ] {
                    let stride = frame.stride(plane);
                    let data = frame.data(plane);
                    let row = size.0 as usize;
                    let mut top = Vec::with_capacity(row * size.1 as usize);
                    for y in 0..size.1 as usize {
                        let line = &data[y * stride..y * stride + row * 2];
                        top.extend(
                            line.as_chunks::<2>()
                                .0
                                .iter()
                                .map(|s| (u16::from_le_bytes(*s) >> shift) as u8),
                        );
                    }
                    write_plane(&gpu.queue, texture, &top, row, size, 1);
                }
            }
            (Layout::Nv12, _) => {
                put(&planes.y, 0, (w, h), 1);
                put(&planes.u, 1, (cw, ch), 2);
            }
            (Layout::P010 | Layout::P210 | Layout::P416, true) => {
                put(&planes.y, 0, (w, h), 2);
                put(&planes.u, 1, (cw, ch), 4);
            }
            (Layout::P010 | Layout::P210 | Layout::P416, false) => {
                for (texture, plane, size, samples) in
                    [(&planes.y, 0, (w, h), 1), (&planes.u, 1, (cw, ch), 2)]
                {
                    let stride = frame.stride(plane);
                    let data = frame.data(plane);
                    let row = (size.0 * samples) as usize;
                    let mut high = Vec::with_capacity(row * size.1 as usize);
                    for y in 0..size.1 as usize {
                        let line = &data[y * stride..y * stride + row * 2];
                        high.extend(line.as_chunks::<2>().0.iter().map(|s| s[1]));
                    }
                    write_plane(&gpu.queue, texture, &high, row, size, samples);
                }
            }
        }
    }
}

impl Screen {
    pub fn read(&self, gpu: &Gpu) -> Option<Image> {
        let target = self.target.as_ref()?;
        let (w, h) = target.size;
        let row = w * 4;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vidcut screenshot"),
            size: u64::from(padded) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("vidcut screenshot"),
            });
        encoder.copy_texture_to_buffer(
            target.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        let submission = gpu.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), wgpu::BufferAsyncError>>();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        gpu.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(std::time::Duration::from_secs(5)),
            })
            .ok()?;
        rx.recv().ok()?.ok()?;
        let mapped = slice.get_mapped_range().ok()?;
        let mut rgba = Vec::with_capacity((row * h) as usize);
        for y in 0..h as usize {
            let start = y * padded as usize;
            rgba.extend_from_slice(&mapped[start..start + row as usize]);
        }
        drop(mapped);
        buffer.unmap();
        Some(Image {
            width: w,
            height: h,
            rgba,
        })
    }
}

fn make_target(device: &wgpu::Device, size: (u32, u32)) -> Target {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vidcut viewer"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TARGET,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    Target {
        texture,
        view,
        size,
    }
}

#[cfg(windows)]
fn d3d11_shared_for(
    gpu: &Gpu,
    pool: &mut Vec<D3d11Shared>,
    frame: &vidcut_av::ffmpeg::frame::Video,
    formats: D3d11Formats,
) -> Result<(usize, bool), String> {
    let free = |pool: &[D3d11Shared]| {
        pool.iter()
            .position(|s| s.texture.fits(frame) && !s.busy.load(Ordering::Acquire))
    };
    if let Some(i) = free(pool) {
        return Ok((i, false));
    }
    if pool.iter().filter(|s| s.texture.fits(frame)).count() >= MAX_SHARED {
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        return free(pool)
            .map(|i| (i, false))
            .ok_or_else(|| "no shared texture came free".into());
    }
    if pool.len() >= 2 * MAX_SHARED
        && let Some(oldest) = pool
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.busy.load(Ordering::Acquire))
            .min_by_key(|(_, s)| s.shown)
            .map(|(i, _)| i)
    {
        pool.remove(oldest);
    }
    pool.push(share_d3d11(gpu, frame, formats)?);
    vidcut_av::debug!(
        "viewer",
        "imported a {}x{} {:?} Direct3D 11 texture to show pictures through ({} in all)",
        frame.width(),
        frame.height(),
        formats.0,
        pool.len()
    );
    Ok((pool.len() - 1, true))
}

#[cfg(windows)]
fn share_d3d11(
    gpu: &Gpu,
    frame: &vidcut_av::ffmpeg::frame::Video,
    (format, one, two): D3d11Formats,
) -> Result<D3d11Shared, String> {
    use ::windows::Win32::Foundation::CloseHandle;
    use ::windows::Win32::Graphics::Direct3D11::{D3D11_TEXTURE2D_DESC, ID3D11Texture2D};
    use ::windows::Win32::Graphics::Dxgi::{DXGI_SHARED_RESOURCE_READ, IDXGIResource1};
    use ::windows::core::Interface;
    use wgpu::hal::api::Vulkan;
    let shared = vidcut_av::hw::d3d11::Shared::new(frame).map_err(|e| e.to_string())?;
    let raw_texture = shared.texture();
    // SAFETY: the shared texture, alive as long as `shared`.
    let texture = unsafe { ID3D11Texture2D::from_raw_borrowed(&raw_texture) }
        .ok_or("a shared picture without a texture")?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: fills the description it is given.
    unsafe { texture.GetDesc(&mut desc) };
    let size = wgpu::Extent3d {
        width: desc.Width,
        height: desc.Height,
        depth_or_array_layers: 1,
    };
    let resource: IDXGIResource1 = texture.cast().map_err(|e| e.to_string())?;
    // SAFETY: a texture made shareable by NT handles.
    let handle = unsafe { resource.CreateSharedHandle(None, DXGI_SHARED_RESOURCE_READ.0, None) }
        .map_err(|e| format!("sharing a Direct3D 11 texture: {e}"))?;
    let label = Some("vidcut Direct3D 11 picture");
    // SAFETY: `handle` is the texture's, as described, and stays open until the import is done.
    let raw = unsafe {
        let raw = gpu
            .device
            .as_hal::<Vulkan>()
            .ok_or("not Vulkan")
            .map(|hal| {
                hal.texture_from_d3d11_shared_handle(
                    ::windows::Win32::Foundation::HANDLE(handle.0),
                    &wgpu::hal::TextureDescriptor {
                        label,
                        size,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUses::RESOURCE,
                        memory_flags: wgpu::hal::MemoryFlags::empty(),
                        view_formats: Vec::new(),
                    },
                )
            });
        let _ = CloseHandle(handle);
        raw?.map_err(|e| format!("importing a Direct3D 11 texture: {e}"))?
    };
    let (image, y, uv) = gpu.checked(|| {
        // SAFETY: `raw` is this device's, as described; acquired into `RESOURCE` before each use.
        let imported = unsafe {
            gpu.device.create_texture_from_hal::<Vulkan>(
                raw,
                &wgpu::TextureDescriptor {
                    label,
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::TextureUses::RESOURCE,
            )
        };
        // SAFETY: wgpu's own texture; only its handle is read.
        let image = unsafe { imported.as_hal::<Vulkan>().map(|t| t.raw_handle()) };
        let plane = |format, aspect| {
            imported.create_view(&wgpu::TextureViewDescriptor {
                label,
                format: Some(format),
                aspect,
                ..Default::default()
            })
        };
        (
            image,
            plane(one, wgpu::TextureAspect::Plane0),
            plane(two, wgpu::TextureAspect::Plane1),
        )
    })?;
    let image = image.ok_or("not Vulkan")?;
    // SAFETY: this device's new image, which nothing has used yet.
    let released = unsafe {
        gpu.release(
            &[image],
            (
                ash::vk::ImageLayout::UNDEFINED,
                ash::vk::ImageLayout::GENERAL,
            ),
            ash::vk::PipelineStageFlags::TOP_OF_PIPE,
            ash::vk::AccessFlags::empty(),
        )?
    };
    let index = gpu.submit([released]);
    gpu.device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(index),
            timeout: None,
        })
        .map_err(|e| format!("handing a texture to Direct3D 11: {e}"))?;
    Ok(D3d11Shared {
        texture: shared,
        image,
        y,
        uv,
        size: (desc.Width, desc.Height),
        busy: Arc::default(),
        shown: std::time::Instant::now(),
    })
}

#[cfg(any(target_os = "linux", windows))]
fn import_vulkan_frame(
    gpu: &Gpu,
    frame: &vidcut_av::ffmpeg::frame::Video,
) -> Result<super::vulkan::Import, String> {
    use wgpu::hal::api::Vulkan;
    let exported = vidcut_av::hw::vulkan::Exported::of(frame).map_err(|e| e.to_string())?;
    if !(3..=4).contains(&exported.planes.len()) {
        return Err(format!("a picture of {} planes", exported.planes.len()));
    }
    let families = exported.planes[0].families.clone();
    let concurrent = families.len() > 1;
    let label = Some("vidcut Vulkan plane");
    // SAFETY: `hal` is wgpu's live device, on the GPU and driver of FFmpeg's (checked first).
    let raws = unsafe {
        let hal = gpu.device.as_hal::<Vulkan>().ok_or("not Vulkan")?;
        if super::vulkan::uuids(&hal) != (exported.device_uuid, exported.driver_uuid) {
            return Err("FFmpeg's Vulkan device isn't on the display's GPU".into());
        }
        if concurrent && !families.contains(&hal.queue_family_index()) {
            return Err("FFmpeg's planes aren't shared with the display's queue".into());
        }
        let imported = exported
            .planes
            .into_iter()
            .map(|plane| super::vulkan::import(&hal, plane))
            .collect::<Result<Vec<_>, _>>()?;
        imported
            .into_iter()
            .map(|plane| plane.into_hal(&hal, label))
            .collect::<Vec<_>>()
    };
    let textures = gpu.checked(|| {
        raws.into_iter()
            .map(|(raw, size, format)| {
                // SAFETY: `raw` is this device's, as described; draws acquire it into `RESOURCE`.
                unsafe {
                    gpu.device.create_texture_from_hal::<Vulkan>(
                        raw,
                        &wgpu::TextureDescriptor {
                            label,
                            size,
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format,
                            usage: wgpu::TextureUsages::TEXTURE_BINDING,
                            view_formats: &[],
                        },
                        wgpu::TextureUses::RESOURCE,
                    )
                }
            })
            .collect::<Vec<_>>()
    })?;
    // SAFETY: wgpu's own textures; only their handles are read.
    let images: Vec<_> = unsafe {
        textures
            .iter()
            .filter_map(|t| t.as_hal::<Vulkan>().map(|t| t.raw_handle()))
            .collect()
    };
    if images.len() != textures.len() {
        return Err("not Vulkan".into());
    }
    let [y, u, v, ..] = &textures[..] else {
        return Err(format!("a picture of {} planes", textures.len()));
    };
    let view = |t: &wgpu::Texture| t.create_view(&Default::default());
    Ok(super::vulkan::Import {
        images,
        views: [view(y), view(u), view(v)],
        size: (y.width(), y.height()),
        concurrent,
        _textures: textures,
    })
}

fn write_plane(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    data: &[u8],
    stride: usize,
    size: (u32, u32),
    bytes: u32,
) {
    let row = (size.0 * bytes) as usize;
    let needed = stride * (size.1 as usize).saturating_sub(1) + row;
    if data.len() < needed {
        return;
    }
    queue.write_texture(
        texture.as_image_copy(),
        &data[..needed],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(stride as u32),
            rows_per_image: Some(size.1),
        },
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
    );
}

const PARAMS: usize = 10 * 16;

fn sixteen(layout: Layout) -> bool {
    match layout {
        Layout::Yuv420p | Layout::Nv12 | Layout::Yuv422p => false,
        Layout::P010
        | Layout::Yuv420p10
        | Layout::Yuv422p10
        | Layout::P210
        | Layout::Yuv444p12
        | Layout::P416 => true,
    }
}

fn interleaved(layout: Layout) -> bool {
    match layout {
        Layout::Nv12 | Layout::P010 | Layout::P210 | Layout::P416 => true,
        Layout::Yuv420p
        | Layout::Yuv420p10
        | Layout::Yuv422p
        | Layout::Yuv422p10
        | Layout::Yuv444p12 => false,
    }
}

fn per_chroma(layout: Layout) -> (u32, u32) {
    match layout {
        Layout::Yuv420p | Layout::Nv12 | Layout::P010 | Layout::Yuv420p10 => (2, 2),
        Layout::Yuv422p | Layout::Yuv422p10 | Layout::P210 => (2, 1),
        Layout::Yuv444p12 | Layout::P416 => (1, 1),
    }
}

fn chroma_size(layout: Layout, w: u32, h: u32) -> (u32, u32) {
    let (across, down) = per_chroma(layout);
    (w.div_ceil(across), h.div_ceil(down))
}

fn params_of(picture: &Picture, size: (u32, u32), wide: bool, cover: (f64, f64)) -> Vec<u8> {
    let colors = shader_colors(picture.colors());
    let (x, y, w, h) = picture.placement(size.0, size.1);
    let turns = u32::from(picture.rotation() / 90) % 4;
    let (sw, sh) = (picture.width(), picture.height());
    let upright = if turns % 2 == 1 { (sh, sw) } else { (sw, sh) };
    let layout = picture.layout();
    let (code, max): (f64, f64) = match (layout, wide && sixteen(layout)) {
        (Layout::P010 | Layout::P210, true) => (65535.0 / 64.0, 1023.0),
        (Layout::Yuv420p10 | Layout::Yuv422p10, true) => (65535.0, 1023.0),
        (Layout::Yuv444p12, true) => (65535.0, 4095.0),
        (Layout::P416, true) => (65535.0, 65535.0),
        _ => (255.0, 255.0),
    };
    let unit = (max + 1.0) / 256.0;
    let (y_off, y_span, c_span) = if colors.full_range {
        (0.0, max, max)
    } else {
        (16.0 * unit, 219.0 * unit, 224.0 * unit)
    };
    let c_off = 128.0 * unit;
    let range = [y_off / code, code / y_span, c_off / code, code / c_span];
    let kg = 1.0 - colors.kr - colors.kb;
    let hdr = match colors.hdr {
        None => 0u32,
        Some(Hdr::Pq) => 1,
        Some(Hdr::Hlg) => 2,
    };
    let g = colors.gamut;
    let floats: [[f64; 4]; 8] = [
        [f64::from(x), f64::from(y), f64::from(w), f64::from(h)],
        [f64::from(upright.0), f64::from(upright.1), cover.0, cover.1],
        range,
        [colors.kr, colors.kb, kg, colors.linear_scale],
        colors.tone,
        [g[0][0], g[0][1], g[0][2], 0.0],
        [g[1][0], g[1][1], g[1][2], 0.0],
        [g[2][0], g[2][1], g[2][2], 0.0],
    ];
    let mut out = Vec::with_capacity(PARAMS);
    for v in floats.iter().flatten() {
        out.extend_from_slice(&(*v as f32).to_le_bytes());
    }
    let uv = u32::from(interleaved(layout));
    let (across, down) = per_chroma(layout);
    for v in [hdr, turns, uv, 0, across, down, 0, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Kind, Takes, shown_from_gpu};

    #[test]
    fn a_display_on_cudas_gpu_shows_vulkans_pictures_from_gpu_memory() {
        let takes = Takes {
            shares: true,
            wide: true,
            on_cudas_gpu: true,
            ..Takes::default()
        };
        assert_eq!(shown_from_gpu(&takes), [Kind::Nvdec, Kind::Vulkan]);
    }

    #[test]
    fn a_display_on_another_gpu_than_cudas_doesnt_take_vulkans_pictures() {
        let takes = Takes {
            shares: true,
            wide: true,
            on_cudas_gpu: false,
            ..Takes::default()
        };
        assert!(!shown_from_gpu(&takes).contains(&Kind::Vulkan));
    }

    #[test]
    fn the_shader_compiles() {
        let module = naga::front::wgsl::parse_str(include_str!("picture.wgsl")).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }
}
