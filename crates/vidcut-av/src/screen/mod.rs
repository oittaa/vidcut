#[cfg(windows)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use crate::color::shader_colors;
use crate::cuda::{CudaDevice, PlaneCopy};
use crate::hw::Kind;
use crate::player::{Layout, Picture, Sampling, TitleOver, TitlePicture};
use crate::thumbs::Image;
use vidcut_core::model::{CompositeMode, Hdr, WipeDirection};
use vidcut_core::place::{Level, Placing, chain, lettered};
use vidcut_core::render::{Placed, Wipe};

#[cfg(any(target_os = "linux", windows))]
mod acquire;
mod carried;
mod gate;
pub(crate) mod held;
pub(crate) mod light;
mod out;
mod shared;
#[cfg(any(target_os = "linux", windows))]
mod vulkan;

pub use gate::Gate;
pub use out::Sink;
pub(crate) use out::filled;

pub struct Gpu {
    #[cfg(any(target_os = "linux", windows))]
    acquirer: OnceLock<Result<acquire::Acquirer, String>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    id: u64,
    screen_drawing: Drawing,
    wide_drawing: Drawing,
    adding: wgpu::RenderPipeline,
    adding_scaled: wgpu::RenderPipeline,
    adding_mode: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    below_layout: wgpu::BindGroupLayout,
    lettering: wgpu::RenderPipeline,
    lettering_wide: wgpu::RenderPipeline,
    lettering_layout: wgpu::BindGroupLayout,
    paired: wgpu::RenderPipeline,
    paired_wide: wgpu::RenderPipeline,
    screen: wgpu::RenderPipeline,
    screen_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    shrinking: wgpu::RenderPipeline,
    halving: wgpu::RenderPipeline,
    blank: wgpu::TextureView,
    wide: bool,
    shares: bool,
    dma_buf: bool,
    d3d11: bool,
    #[cfg(windows)]
    p010: bool,
    #[cfg(windows)]
    d3d11_shared: std::sync::Mutex<D3d11Pool>,
    metal: bool,
    cuda: OnceLock<Result<CudaDevice, String>>,
    gate: Option<Arc<Gate>>,
    encoders: out::Encoders,
    held: held::Pipes,
    light: OnceLock<light::Pipes>,
}

struct Drawing {
    pipeline: wgpu::RenderPipeline,
    blended: wgpu::RenderPipeline,
    placed: wgpu::RenderPipeline,
    mode: wgpu::RenderPipeline,
}

const MAX_SHARED: usize = 4;

static IDS: AtomicU64 = AtomicU64::new(1);

const TARGET: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

const MIXING: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

fn checked<T>(device: &wgpu::Device, f: impl FnOnce() -> T) -> Result<T, String> {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let made = f();
    match pollster::block_on(scope.pop()) {
        Some(e) => Err(format!("the display's device refused it: {e}")),
        None => Ok(made),
    }
}

pub fn features(adapter: &wgpu::Adapter) -> wgpu::Features {
    let wanted = wgpu::Features::TEXTURE_FORMAT_16BIT_NORM
        | wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
    #[cfg(target_os = "linux")]
    let wanted = wanted | wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF;
    #[cfg(windows)]
    let wanted = wanted
        | wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32
        | wgpu::Features::TEXTURE_FORMAT_NV12
        | wgpu::Features::TEXTURE_FORMAT_P010;
    adapter.features() & wanted
}

pub struct Offscreen {
    pub adapter: wgpu::AdapterInfo,
    pub gpu: Gpu,
}

type Made = (
    Option<String>,
    std::result::Result<&'static Offscreen, String>,
);

pub fn offscreen() -> std::result::Result<&'static Offscreen, String> {
    static MADE: std::sync::Mutex<Vec<Made>> = std::sync::Mutex::new(Vec::new());
    let wanted = crate::gpus::in_use();
    let key = wanted.as_ref().map(|g| g.id.clone());
    let mut made = MADE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, on)) = made.iter().find(|(k, _)| *k == key) {
        return on.clone();
    }
    let on = device("vidcut offscreen", wanted.as_ref()).map(|(adapter, device, queue)| {
        &*Box::leak(Box::new(Offscreen {
            adapter: adapter.get_info(),
            gpu: Gpu::new(&device, &queue),
        }))
    });
    made.push((key, on.clone()));
    on
}

pub(crate) type Started<N> = (Option<&'static Offscreen>, std::result::Result<N, String>);

pub(crate) fn start_on_offscreen<N>(
    engine: &mut Option<Started<N>>,
    start: impl FnOnce() -> std::result::Result<N, String>,
) -> &mut std::result::Result<N, String> {
    let on = offscreen().ok();
    if engine
        .as_ref()
        .is_some_and(|(was, _)| was.map(std::ptr::from_ref) != on.map(std::ptr::from_ref))
    {
        *engine = None;
    }
    &mut engine.get_or_insert_with(|| (on, start())).1
}

fn adapter_id(adapter: &wgpu::Adapter) -> String {
    let info = adapter.get_info();
    crate::gpus::adapter_id(info.vendor, info.device, &info.device_pci_bus_id)
}

fn device(
    label: &str,
    wanted: Option<&crate::gpus::Gpu>,
) -> std::result::Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue), String> {
    let backends = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY);
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        ..wgpu::InstanceDescriptor::new_without_display_handle_from_env()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference:
            wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::HighPerformance),
        ..Default::default()
    }))
    .map_err(|e| format!("no GPU for wgpu: {e}"))?;
    let adapter = match wanted {
        Some(gpu) if !gpu.is_adapter(&adapter_id(&adapter)) => {
            let backend = adapter.get_info().backend;
            let on_it = pollster::block_on(instance.enumerate_adapters(backends))
                .into_iter()
                .filter(|a| gpu.is_adapter(&adapter_id(a)))
                .min_by_key(|a| a.get_info().backend != backend);
            match on_it {
                Some(a) => a,
                None => {
                    eprintln!(
                        "vidcut: wgpu has no adapter on {}; drawing on {}",
                        gpu.name,
                        adapter.get_info().name
                    );
                    adapter
                }
            }
        }
        _ => adapter,
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some(label),
        required_features: features(&adapter),
        required_limits: adapter.limits(),
        memory_hints: wgpu::MemoryHints::MemoryUsage,
        ..Default::default()
    }))
    .map_err(|e| format!("no wgpu device: {e}"))?;
    Ok((adapter, device, queue))
}

const RESIZE: u32 = 4;

const WIDENS: u32 = 8;

pub fn kernel(sharp: Option<vidcut_core::model::Sharpness>) -> u32 {
    use vidcut_core::model::Sharpness;
    match sharp {
        None => 0,
        Some(Sharpness::Low) => 1,
        Some(Sharpness::Medium) => 2,
        Some(Sharpness::High) => 3,
    }
}

impl Gpu {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vidcut picture"),
            source: wgpu::ShaderSource::Wgsl(shader().into()),
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
                texture(7),
                texture(8),
            ],
        });
        let below_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vidcut below"),
            entries: &[texture(0)],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vidcut picture"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let mode_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vidcut composite"),
            bind_group_layouts: &[Some(&layout), Some(&below_layout)],
            immediate_size: 0,
        });
        let made = |layout: &wgpu::PipelineLayout,
                    entry: &str,
                    blend: Option<wgpu::BlendState>,
                    format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("vidcut picture"),
                layout: Some(layout),
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
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline_in = |blend, format| made(&pipeline_layout, "fs", blend, format);
        let over = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Constant,
            dst_factor: wgpu::BlendFactor::OneMinusConstant,
            operation: wgpu::BlendOperation::Add,
        };
        let premultiplied = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let drawing = |format| Drawing {
            pipeline: pipeline_in(None, format),
            blended: pipeline_in(
                Some(wgpu::BlendState {
                    color: over,
                    alpha: over,
                }),
                format,
            ),
            placed: pipeline_in(
                Some(wgpu::BlendState {
                    color: premultiplied,
                    alpha: premultiplied,
                }),
                format,
            ),
            mode: made(&mode_layout, "fs_mode", None, format),
        };
        let screen_drawing = drawing(TARGET);
        let wide_drawing = drawing(MIXING);
        let add = |src_factor| wgpu::BlendComponent {
            src_factor,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let adding = pipeline_in(
            Some(wgpu::BlendState {
                color: add(wgpu::BlendFactor::One),
                alpha: add(wgpu::BlendFactor::One),
            }),
            MIXING,
        );
        let adding_scaled = pipeline_in(
            Some(wgpu::BlendState {
                color: add(wgpu::BlendFactor::Constant),
                alpha: add(wgpu::BlendFactor::Constant),
            }),
            MIXING,
        );
        let adding_mode = made(
            &mode_layout,
            "fs_mode",
            Some(wgpu::BlendState {
                color: add(wgpu::BlendFactor::One),
                alpha: add(wgpu::BlendFactor::One),
            }),
            MIXING,
        );
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
        let lettering_in = |format| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
                        format,
                        blend: Some(wgpu::BlendState {
                            color: premultiplied,
                            alpha: premultiplied,
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let lettering = lettering_in(TARGET);
        let lettering_wide = lettering_in(MIXING);
        let paired_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vidcut dissolve"),
            bind_group_layouts: &[Some(&lettering_layout), Some(&below_layout)],
            immediate_size: 0,
        });
        let paired = made(&paired_layout, "fs_paired", None, TARGET);
        let paired_wide = made(&paired_layout, "fs_paired", None, MIXING);
        let screen_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vidcut screen"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture(9),
            ],
        });
        let screen_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("vidcut screen"),
                bind_group_layouts: &[Some(&screen_layout)],
                immediate_size: 0,
            });
        let screen = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vidcut screen"),
            layout: Some(&screen_pipeline_layout),
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
                entry_point: Some("fs_screen"),
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("vidcut picture"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shrinking = made(&pipeline_layout, "fs_shrink", None, MIXING);
        let halving = made(&pipeline_layout, "fs_halve", None, MIXING);
        let blank = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("vidcut no halves"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: MIXING,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        Self {
            #[cfg(any(target_os = "linux", windows))]
            acquirer: OnceLock::new(),
            device: device.clone(),
            queue: queue.clone(),
            id: IDS.fetch_add(1, Ordering::Relaxed),
            screen_drawing,
            wide_drawing,
            adding,
            adding_scaled,
            adding_mode,
            layout,
            below_layout,
            lettering,
            lettering_wide,
            lettering_layout,
            paired,
            paired_wide,
            screen,
            screen_layout,
            sampler,
            shrinking,
            halving,
            blank,
            wide: device
                .features()
                .contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM),
            shares: shared::possible(device),
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
            encoders: out::Encoders::new(device, &shader),
            held: held::Pipes::new(device),
            light: OnceLock::new(),
        }
    }

    pub fn behind(mut self, gate: Arc<Gate>) -> Self {
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

    pub(crate) fn handles(&self) -> (&wgpu::Device, &wgpu::Queue) {
        (&self.device, &self.queue)
    }

    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub fn gpu_pictures(&self) -> crate::hw::GpuPictures {
        let cudas = crate::gpus::in_use().and_then(|g| g.uuid());
        shown_from_gpu(&Takes {
            shares: self.shares,
            wide: self.wide,
            on_cudas_gpu: cudas.is_some() && cudas == shared::uuid(&self.device),
            dma_buf: self.dma_buf,
            d3d11: self.d3d11,
            metal: self.metal,
        })
        .into_iter()
        .flat_map(|kind| Layout::ALL.map(|layout| (kind, layout)))
        .filter(|&(kind, layout)| self.imports(kind, layout))
        .collect()
    }

    fn imports(&self, kind: Kind, layout: Layout) -> bool {
        match kind {
            Kind::Nvdec => nvdec_bytes(self, layout).is_ok(),
            #[cfg(target_os = "linux")]
            Kind::Vaapi => vaapi_formats(self, layout).is_ok(),
            #[cfg(windows)]
            Kind::D3d11va => d3d11_formats(self, layout).is_ok(),
            #[cfg(target_os = "macos")]
            Kind::VideoToolbox => videotoolbox_formats(self, layout).is_ok(),
            Kind::Vulkan => vulkan_layout(self, layout).is_ok(),
            _ => false,
        }
    }

    #[cfg(any(target_os = "linux", windows))]
    unsafe fn acquire(
        &self,
        images: &[ash::vk::Image],
        layout: ash::vk::ImageLayout,
        stage: ash::vk::PipelineStageFlags,
        access: ash::vk::AccessFlags,
        source: acquire::Source,
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
        record: impl FnOnce(&acquire::Acquirer, ash::vk::CommandBuffer),
    ) -> Result<wgpu::CommandBuffer, String> {
        let acquirer = self
            .acquirer
            .get_or_init(|| {
                // SAFETY: wgpu's own device, only read here.
                let hal = unsafe { self.device.as_hal::<wgpu::hal::api::Vulkan>() };
                hal.map(|hal| acquire::Acquirer::new(&hal))
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
        checked(&self.device, f)
    }

    fn cuda(&self) -> Result<&CudaDevice, String> {
        self.cuda
            .get_or_init(|| shared::cuda_on(&self.device))
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
        (Kind::Nvdec, takes.shares && takes.on_cudas_gpu),
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
    key: (Layout, u32, u32, bool, bool),
    y: wgpu::Texture,
    u: wgpu::Texture,
    v: wgpu::Texture,
    a: wgpu::Texture,
}

struct Target {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
}

struct Wide {
    size: (u32, u32),
    hdr: Option<Hdr>,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
}

impl Wide {
    fn new(gpu: &Gpu, size: (u32, u32), hdr: Option<Hdr>) -> Self {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vidcut timeline"),
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
        let colors = shader_colors(crate::color::Colors {
            matrix: crate::color::Matrix::Bt2020,
            full_range: false,
            hdr,
        });
        let g = colors.gamut;
        let floats: [[f64; 4]; 5] = [
            colors.tone,
            [g[0][0], g[0][1], g[0][2], 0.0],
            [g[1][0], g[1][1], g[1][2], 0.0],
            [g[2][0], g[2][1], g[2][2], 0.0],
            [transfer_of(hdr), colors.linear_scale, 0.0, 0.0],
        ];
        let bytes: Vec<u8> = floats
            .iter()
            .flatten()
            .flat_map(|v| (*v as f32).to_le_bytes())
            .collect();
        let params = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vidcut timeline"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue.write_buffer(&params, 0, &bytes);
        let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vidcut timeline"),
            layout: &gpu.screen_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        Self {
            size,
            hdr,
            texture,
            view,
            bind,
        }
    }
}

struct Below {
    size: (u32, u32),
    format: wgpu::TextureFormat,
    texture: wgpu::Texture,
    bind: wgpu::BindGroup,
}

impl Below {
    fn new(gpu: &Gpu, size: (u32, u32), format: wgpu::TextureFormat) -> Self {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vidcut below"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vidcut below"),
            layout: &gpu.below_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        Self {
            size,
            format,
            texture,
            bind,
        }
    }
}

fn shown_part(wipe: Wipe, (w, h): (u32, u32)) -> (u32, u32, u32, u32) {
    let (n, d) = wipe.along;
    if d <= 0 {
        return if wipe.falling {
            (0, 0, w, h)
        } else {
            (0, 0, 0, 0)
        };
    }
    let n = n.clamp(0, d);
    let edge = |size: u32, k: i64| {
        ((i128::from(size) * i128::from(k) + i128::from(d) - 1) / i128::from(d)) as u32
    };
    let ((x0, x1), (y0, y1)) = match wipe.direction {
        WipeDirection::Left => ((0, edge(w, n)), (0, h)),
        WipeDirection::Right => ((edge(w, d - n), w), (0, h)),
        WipeDirection::Up => ((0, w), (0, edge(h, n))),
        WipeDirection::Down => ((0, w), (edge(h, d - n), h)),
    };
    let flip = |(a, b): (u32, u32), size: u32| if a == 0 { (b, size) } else { (0, a) };
    let ((x0, x1), (y0, y1)) = match (wipe.falling, wipe.direction) {
        (false, _) => ((x0, x1), (y0, y1)),
        (true, WipeDirection::Left | WipeDirection::Right) => (flip((x0, x1), w), (y0, y1)),
        (true, WipeDirection::Up | WipeDirection::Down) => ((x0, x1), flip((y0, y1), h)),
    };
    (x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
}

fn transfer_of(colors: Option<Hdr>) -> f64 {
    match colors {
        None => 0.0,
        Some(Hdr::Pq) => 1.0,
        Some(Hdr::Hlg) => 2.0,
    }
}

#[derive(Default)]
pub struct Screen {
    target: Option<Target>,
    wide: Option<Wide>,
    drawn_colors: Option<Hdr>,
    mixing: Option<Mixing>,
    below: Option<Below>,
    drawn_crosses: Vec<bool>,
    drawn_kernels: Vec<u32>,
    drawn: Option<Option<Arc<Picture>>>,
    drawn_titles: Vec<TitleOver>,
    drawn_placed: Vec<Vec<Placed>>,
    slots: Vec<Slot>,
    letters: Vec<Letters>,
    imports_before: u64,
    exported: bool,
    encoded: out::Encoded,
    spent: (std::time::Duration, std::time::Duration),
}

struct Letters {
    picture: Arc<TitlePicture>,
    params: wgpu::Buffer,
    bind: wgpu::BindGroup,
}

struct Mixing {
    size: (u32, u32),
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
}

impl Mixing {
    fn new(gpu: &Gpu, size: (u32, u32)) -> Self {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vidcut dissolve"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: MIXING,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let params = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vidcut dissolve"),
            size: LETTERING as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (w, h) = (f64::from(size.0), f64::from(size.1));
        let whole = [1.0 / w, 0.0, 0.0, 0.0, 1.0 / h, 0.0, 0.0, 0.0];
        gpu.queue
            .write_buffer(&params, 0, &lettering_of(whole, 1.0, None, None));
        let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vidcut dissolve"),
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
        Self { size, view, bind }
    }
}

fn begin<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    view: &'a wgpu::TextureView,
    clear: Option<wgpu::Color>,
) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("vidcut picture"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

fn letter(
    pass: &mut wgpu::RenderPass<'_>,
    lettering: &wgpu::RenderPipeline,
    letters: &[Letters],
    titles: &[TitleOver],
    after: usize,
    layers: usize,
) {
    for t in titles
        .iter()
        .filter(|t| t.above == after || (after == layers && t.above > after))
    {
        let Some(l) = letters.iter().find(|l| Arc::ptr_eq(&l.picture, &t.picture)) else {
            continue;
        };
        pass.set_pipeline(lettering);
        pass.set_bind_group(0, &l.bind, &[]);
        pass.draw(0..3, 0..1);
    }
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
            size: LETTERING as u64,
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

    fn place(&self, gpu: &Gpu, size: (u32, u32), t: &TitleOver, colors: Option<Hdr>) {
        let p = &self.picture;
        let (fw, fh) = (f64::from(p.frame.0.max(1)), f64::from(p.frame.1.max(1)));
        let (sx, sy) = (f64::from(size.0) / fw, f64::from(size.1) / fh);
        let (w, h) = (
            f64::from(p.width.max(1)) * sx,
            f64::from(p.height.max(1)) * sy,
        );
        let own = lettered(
            [f64::from(t.turn[0]) * sx, f64::from(t.turn[1]) * sy],
            f64::from(t.turn[2]),
            f64::from(t.turn[3]),
            [f64::from(p.left + t.dx) * sx, f64::from(p.top + t.dy) * sy],
        );
        let own = chain(&own, &[1.0 / w, 0.0, 0.0, 0.0, 1.0 / h, 0.0, 0.0, 0.0]);
        let frame = (f64::from(size.0), f64::from(size.1));
        let placing = (!t.placed.is_empty()).then(|| Placing::whole(&t.placed, frame));
        let map = placing
            .as_ref()
            .map_or(own, |l| chain(&l.inner(frame), &own));
        let bytes = lettering_of(map, f64::from(t.alpha), placing.as_ref(), colors);
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
                && x.placed == y.placed
                && Arc::ptr_eq(&x.picture, &y.picture)
        })
}

#[derive(Default)]
struct Slot {
    planes: Option<Planes>,
    params: Option<wgpu::Buffer>,
    shared: Vec<shared::Shared>,
    filled: Option<usize>,
    unshared: bool,
    unimported: Vec<Layout>,
    source: Option<Arc<crate::decode::Decoding>>,
    imported: Option<Arc<Picture>>,
    views: Option<(wgpu::TextureView, wgpu::TextureView, (u32, u32))>,
    v_view: Option<wgpu::TextureView>,
    acquired: Option<wgpu::CommandBuffer>,
    #[cfg(any(target_os = "linux", windows))]
    kept: vulkan::Imports,
    #[cfg(any(target_os = "linux", windows))]
    computes: bool,
    released: Option<wgpu::CommandBuffer>,
    #[cfg(windows)]
    d3d11_used: Option<Arc<AtomicBool>>,
    imports: u64,
    last_shared: bool,
    written: Option<Arc<Picture>>,
    rgb: Option<wgpu::Buffer>,
    shrunk: Option<(wgpu::Texture, (u32, u32, u32))>,
}

#[cfg(windows)]
struct D3d11Shared {
    texture: crate::hw::d3d11::Shared,
    image: ash::vk::Image,
    y: wgpu::TextureView,
    uv: wgpu::TextureView,
    size: (u32, u32),
    busy: Arc<AtomicBool>,
    holds: std::sync::Weak<Picture>,
}

#[cfg(windows)]
type D3d11Pool = vidcut_core::lru::Lru<ash::vk::Image, D3d11Shared, { usize::MAX }>;

#[cfg(windows)]
type D3d11Formats = (
    wgpu::TextureFormat,
    wgpu::TextureFormat,
    wgpu::TextureFormat,
);

pub type Layers<'a> = (&'a [Vec<Placed>], &'a [bool], &'a [u32], Option<Hdr>);

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
        (placed, crosses, kernels, colors): Layers<'_>,
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
            } && same_titles(&self.drawn_titles, titles)
                && self.drawn_placed == placed
                && self.drawn_crosses == crosses
                && self.drawn_kernels == kernels
                && self.drawn_colors == colors;
            if same {
                return Drawn::Same;
            }
        }
        let limit = gpu.device.limits().max_texture_dimension_2d;
        let fits = |p: &Arc<Picture>| p.width() <= limit && p.height() <= limit;
        let picture = picture.filter(|p| fits(p));
        let layers: Vec<(&Arc<Picture>, Option<f64>, Option<Wipe>)> = picture
            .into_iter()
            .map(|p| (p, None, None))
            .chain(picture.into_iter().flat_map(|p| {
                p.over()
                    .iter()
                    .filter(|o| fits(&o.picture))
                    .map(|o| match o.wipe {
                        Some(w) => (&o.picture, None, Some(w)),
                        None => (&o.picture, Some(f64::from(o.level())), None),
                    })
            }))
            .collect();
        if self.slots.len() < layers.len() {
            self.slots.resize_with(layers.len(), Slot::default);
        }
        #[cfg(any(target_os = "linux", windows))]
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if layers
                .get(i)
                .is_none_or(|(p, _, _)| p.gpu() != Some(Kind::Vulkan))
            {
                slot.kept.clear();
            }
        }
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("vidcut picture"),
            });
        let n = layers.len();
        let wipe_of = |i: usize| layers.get(i).and_then(|l| l.2);
        let crossed = |i: usize| crosses.get(i).copied().unwrap_or(false);
        let crossed_wipe = |i: usize| wipe_of(i).is_some() && crossed(i);
        let holds = |i: usize| i + 1 < n && crossed_wipe(i + 1);
        let part = |i: usize| match wipe_of(i) {
            Some(w) if !holds(i) => Some(shown_part(w, size)),
            _ => None,
        };
        let pair_at = |i: usize| i + 1 < n && crossed(i + 1) && wipe_of(i + 1).is_none();
        let mut paired = vec![false; n];
        let mut i = 0;
        while i < n {
            if pair_at(i) {
                paired[i..i + 2].fill(true);
                i += 2;
            } else {
                i += 1;
            }
        }
        let exported = self.exported;
        let clock = std::time::Instant::now();
        let binds: Vec<(wgpu::BindGroup, Option<f64>, bool, bool)> = layers
            .iter()
            .zip(&mut self.slots)
            .enumerate()
            .map(|(i, (&(picture, level, _), slot))| {
                let made_with = match picture.sampling() {
                    Sampling::Placed { placed, .. } => Some(&placed[..]),
                    _ => None,
                };
                let placing = made_with
                    .or(placed.get(i).map(Vec::as_slice))
                    .filter(|p| !p.is_empty())
                    .map(|p| {
                        let (frame, fitted) = picture.placed_in(size);
                        (Placing::new(p, frame, fitted), level.unwrap_or(1.0))
                    });
                let colors_only = matches!(picture.sampling(), Sampling::Colors { .. });
                let blended = !colors_only
                    && placing
                        .as_ref()
                        .is_some_and(|(p, _)| p.mode != CompositeMode::Normal);
                let kernel = match kernels.get(i).copied().unwrap_or(0) {
                    0 if exported => RESIZE | WIDENS,
                    k if exported => k | WIDENS,
                    k => k,
                };
                let how = (placing.as_ref(), kernel, colors, paired[i]);
                let bind = slot.prepare(gpu, picture, size, how, &mut encoder);
                (bind, level, placing.is_some(), blended)
            })
            .collect();
        self.spent.0 += clock.elapsed();
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
                l.place(gpu, size, t, colors);
            }
        }
        if (0..n).any(pair_at) && self.mixing.as_ref().is_none_or(|m| m.size != size) {
            self.mixing = Some(Mixing::new(gpu, size));
        }
        let blending = binds.iter().any(|b| b.3);
        let floats = self.exported || colors.is_some() || blending || n > 1;
        if !floats {
            self.wide = None;
        } else if self
            .wide
            .as_ref()
            .is_none_or(|w| w.size != size || w.hdr != colors)
        {
            self.wide = Some(Wide::new(gpu, size, colors));
        }
        let format = if floats { MIXING } else { TARGET };
        if (blending || (0..n).any(holds))
            && self
                .below
                .as_ref()
                .is_none_or(|b| b.size != size || b.format != format)
        {
            self.below = Some(Below::new(gpu, size, format));
        }
        let target = self.target.as_ref().expect("made above");
        let wide = self.wide.as_ref();
        let canvas = wide.map_or(&target.view, |w| &w.view);
        let canvas_texture = wide.map_or(&target.texture, |w| &w.texture);
        let blends = |i: usize| binds.get(i).is_some_and(|b| b.3);
        let (drawing, lettering, paired) = match wide {
            Some(_) => (&gpu.wide_drawing, &gpu.lettering_wide, &gpu.paired_wide),
            None => (&gpu.screen_drawing, &gpu.lettering, &gpu.paired),
        };
        let mut clear = Some(wgpu::Color::BLACK);
        if n == 0 {
            drop(begin(&mut encoder, &target.view, clear));
            drop(begin(&mut encoder, canvas, clear.take()));
        }
        let level_of = |l: f64| wgpu::Color {
            r: l,
            g: l,
            b: l,
            a: l,
        };
        let whole = wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        };
        let starts = |i: usize| blends(i) || crossed_wipe(i) || holds(i);
        let mut i = 0;
        while i < n {
            if crossed_wipe(i) || holds(i) {
                if clear.is_some() {
                    drop(begin(&mut encoder, canvas, clear.take()));
                }
                let below = self.below.as_ref().expect("made for a wipe");
                if let Some((x, y, w, h)) = part(i).filter(|p| p.2 > 0 && p.3 > 0) {
                    let at = wgpu::Origin3d { x, y, z: 0 };
                    encoder.copy_texture_to_texture(
                        wgpu::TexelCopyTextureInfo {
                            origin: at,
                            ..below.texture.as_image_copy()
                        },
                        wgpu::TexelCopyTextureInfo {
                            origin: at,
                            ..canvas_texture.as_image_copy()
                        },
                        wgpu::Extent3d {
                            width: w,
                            height: h,
                            depth_or_array_layers: 1,
                        },
                    );
                }
                if holds(i) {
                    encoder.copy_texture_to_texture(
                        canvas_texture.as_image_copy(),
                        below.texture.as_image_copy(),
                        whole,
                    );
                }
            }
            if (blends(i) || (pair_at(i) && blends(i + 1))) && !holds(i) {
                if clear.is_some() {
                    drop(begin(&mut encoder, canvas, clear.take()));
                }
                let below = self.below.as_ref().expect("made for a composite mode");
                encoder.copy_texture_to_texture(
                    canvas_texture.as_image_copy(),
                    below.texture.as_image_copy(),
                    wgpu::Extent3d {
                        width: size.0,
                        height: size.1,
                        depth_or_array_layers: 1,
                    },
                );
            }
            if pair_at(i) {
                let mixing = self.mixing.as_ref().expect("made for a dissolve");
                {
                    let mut pass =
                        begin(&mut encoder, &mixing.view, Some(wgpu::Color::TRANSPARENT));
                    for (bind, level, placed, blended) in &binds[i..i + 2] {
                        if *blended {
                            let below = self.below.as_ref().expect("made for a composite mode");
                            pass.set_pipeline(&gpu.adding_mode);
                            pass.set_bind_group(1, &below.bind, &[]);
                        } else if *placed {
                            pass.set_pipeline(&gpu.adding);
                        } else {
                            pass.set_pipeline(&gpu.adding_scaled);
                            pass.set_blend_constant(level_of(level.unwrap_or(1.0)));
                        }
                        pass.set_bind_group(0, bind, &[]);
                        pass.draw(0..3, 0..1);
                    }
                }
                let mut pass = begin(&mut encoder, canvas, clear.take());
                if blends(i) || blends(i + 1) {
                    let below = self.below.as_ref().expect("made for a composite mode");
                    pass.set_pipeline(paired);
                    pass.set_bind_group(1, &below.bind, &[]);
                } else {
                    pass.set_pipeline(lettering);
                }
                pass.set_bind_group(0, &mixing.bind, &[]);
                pass.draw(0..3, 0..1);
                letter(&mut pass, lettering, &self.letters, titles, i + 1, n);
                letter(&mut pass, lettering, &self.letters, titles, i + 2, n);
                i += 2;
                continue;
            }
            let mut pass = begin(&mut encoder, canvas, clear.take());
            let first = i;
            while i < n && !pair_at(i) && (i == first || !starts(i)) {
                let (bind, level, placed, blended) = &binds[i];
                let shown = part(i);
                if shown.is_none_or(|p| p.2 > 0 && p.3 > 0) {
                    if let Some((x, y, w, h)) = shown {
                        pass.set_scissor_rect(x, y, w, h);
                    }
                    match level {
                        _ if *blended => {
                            let below = self.below.as_ref().expect("made for a composite mode");
                            pass.set_pipeline(&drawing.mode);
                            pass.set_bind_group(1, &below.bind, &[]);
                        }
                        _ if *placed => pass.set_pipeline(&drawing.placed),
                        None => pass.set_pipeline(&drawing.pipeline),
                        Some(l) => {
                            pass.set_pipeline(&drawing.blended);
                            pass.set_blend_constant(level_of(*l));
                        }
                    }
                    pass.set_bind_group(0, bind, &[]);
                    pass.draw(0..3, 0..1);
                    if shown.is_some() {
                        pass.set_scissor_rect(0, 0, size.0, size.1);
                    }
                }
                letter(&mut pass, lettering, &self.letters, titles, i + 1, n);
                i += 1;
            }
        }
        if let Some(w) = wide
            && n > 0
            && !self.exported
        {
            let mut pass = begin(&mut encoder, &target.view, Some(wgpu::Color::BLACK));
            pass.set_pipeline(&gpu.screen);
            pass.set_bind_group(0, &w.bind, &[]);
            pass.draw(0..3, 0..1);
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
        self.drawn_placed = placed.to_vec();
        self.drawn_crosses = crosses.to_vec();
        self.drawn_kernels = kernels.to_vec();
        self.drawn_colors = colors;
        drawn
    }
}

impl Slot {
    fn prepare(
        &mut self,
        gpu: &Gpu,
        picture: &Arc<Picture>,
        size: (u32, u32),
        how: (Option<&(Placing, f64)>, u32, Option<Hdr>, bool),
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
        let halvings = halvings(picture, size, how);
        let bytes = params_of(picture, size, gpu.wide, cover, how, halvings);
        gpu.queue.write_buffer(params, 0, &bytes);
        let planes = self.planes.as_ref().expect("uploaded");
        let view = |t: &wgpu::Texture| t.create_view(&Default::default());
        let (y, u) = self
            .views
            .take()
            .map(|(y, u, _)| (y, u))
            .unwrap_or_else(|| (view(&planes.y), view(&planes.u)));
        let v = self.v_view.take().unwrap_or_else(|| view(&planes.v));
        let a = view(&planes.a);
        let bind = |halved: &wgpu::TextureView| {
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
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(&a),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::TextureView(halved),
                    },
                ],
            })
        };
        if halvings == 0 {
            return bind(&gpu.blank);
        }
        let wanted = (
            picture.width().div_ceil(2),
            picture.height().div_ceil(2),
            halvings,
        );
        if self.shrunk.as_ref().is_none_or(|(_, k)| *k != wanted) {
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("vidcut halves"),
                size: wgpu::Extent3d {
                    width: wanted.0,
                    height: wanted.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: halvings,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: MIXING,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            self.shrunk = Some((texture, wanted));
        }
        let shrunk = &self.shrunk.as_ref().expect("made above").0;
        let level = |k: u32| {
            shrunk.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: k,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        for k in 0..halvings {
            let (pipeline, from) = match k {
                0 => (&gpu.shrinking, bind(&gpu.blank)),
                _ => (&gpu.halving, bind(&level(k - 1))),
            };
            let to = level(k);
            let mut pass = begin(encoder, &to, Some(wgpu::Color::TRANSPARENT));
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &from, &[]);
            pass.draw(0..3, 0..1);
        }
        bind(&shrunk.create_view(&Default::default()))
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
        if self.source.as_ref().map(Arc::as_ptr) != picture.decoding().map(Arc::as_ptr) {
            self.source = picture.decoding().cloned();
            self.unshared = false;
            self.unimported.clear();
        }
        self.make_planes(gpu, picture);
        let written = self.written.take();
        if picture.gpu().is_some() && written.as_ref().is_some_and(|w| Arc::ptr_eq(w, picture)) {
            self.written = written;
            return;
        }
        self.last_shared = false;
        match picture.gpu() {
            None => {
                if !written.is_some_and(|w| same_data(&w, picture)) {
                    self.write_planes(gpu, picture, encoder);
                }
                self.written = Some(Arc::clone(picture));
                return;
            }
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
                return self.copy_down(gpu, picture, encoder);
            }
        }
        if !self.unshared {
            match self.copy_on_gpu(gpu, picture, encoder) {
                Ok(true) => {
                    self.last_shared = true;
                    self.written = Some(Arc::clone(picture));
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
        self.copy_down(gpu, picture, encoder);
    }

    fn copy_down(&mut self, gpu: &Gpu, picture: &Arc<Picture>, encoder: &mut wgpu::CommandEncoder) {
        match picture.alone_copied_down() {
            Ok(down) => {
                self.write_planes(gpu, &down, encoder);
                self.written = Some(Arc::clone(picture));
            }
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
        let (one, two) = vaapi_formats(gpu, picture.layout())?;
        let drm = crate::hw::vaapi::DrmFrame::export(picture.frame()).map_err(|e| e.to_string())?;
        let planes = drm.planes();
        let [y, uv, ..] = planes[..] else {
            return Err(format!("a surface exported in {} planes", planes.len()));
        };
        let (w, h) = (picture.width(), picture.height());
        let ours = self.planes.as_ref().expect("made by upload");
        let label = Some("vidcut VA-API plane");
        let mut held = HeldPlanes {
            gpu,
            raws: Vec::new(),
        };
        let mut specs = Vec::new();
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
            held.raws.push(raw);
            specs.push((size, format, texture));
        }
        let textures = gpu.checked(|| {
            held.raws
                .drain(..)
                .zip(specs)
                .map(|(raw, (size, format, texture))| {
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
                acquire::Source::Api,
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

    #[cfg(any(target_os = "linux", windows))]
    fn reading_stage(&self) -> ash::vk::PipelineStageFlags {
        if self.computes {
            ash::vk::PipelineStageFlags::COMPUTE_SHADER
        } else {
            ash::vk::PipelineStageFlags::FRAGMENT_SHADER
        }
    }

    #[cfg(windows)]
    fn import_d3d11(&mut self, gpu: &Gpu, picture: &Arc<Picture>) -> Result<(), String> {
        use ash::vk::{AccessFlags, ImageLayout};
        if !gpu.d3d11 {
            return Err("the display's device imports no Direct3D 11 textures".into());
        }
        let formats = d3d11_formats(gpu, picture.layout())?;
        let frame = picture.frame();
        let mut pool = gpu.d3d11_shared.lock().unwrap_or_else(|e| e.into_inner());
        let (image, new) = d3d11_shared_for(gpu, &mut pool, picture, formats)?;
        if new {
            self.imports += 1;
        }
        let shared = pool.get_mut(&image).ok_or("no shared texture")?;
        if !std::ptr::eq(shared.holds.as_ptr(), Arc::as_ptr(picture)) {
            shared.texture.fill(frame).map_err(|e| e.to_string())?;
            shared.holds = Arc::downgrade(picture);
        }
        let stage = self.reading_stage();
        // SAFETY: this device's image, released to Direct3D 11 in `GENERAL`, which wrote it last.
        let (acquired, released) = unsafe {
            (
                gpu.acquire(
                    &[shared.image],
                    ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    stage,
                    AccessFlags::SHADER_READ,
                    acquire::Source::Api,
                )?,
                gpu.release(
                    &[shared.image],
                    (ImageLayout::SHADER_READ_ONLY_OPTIMAL, ImageLayout::GENERAL),
                    stage,
                    AccessFlags::SHADER_READ,
                )?,
            )
        };
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
        vulkan_layout(gpu, picture.layout())?;
        let frame = picture.frame();
        let identity = crate::hw::vulkan::Identity::of(frame).map_err(|e| e.to_string())?;
        let stage = self.reading_stage();
        let (import, new) = self
            .kept
            .get_or_import(identity, || import_vulkan_frame(gpu, frame))?;
        let left_in = crate::hw::vulkan::layout(frame).map_err(|e| e.to_string())?;
        // SAFETY: this device's images on FFmpeg's memory; wgpu left any it used shader-readable.
        let acquired = unsafe {
            gpu.acquire(
                &import.images,
                ash::vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                stage,
                ash::vk::AccessFlags::SHADER_READ,
                acquire::Source::Vulkan {
                    concurrent: import.concurrent,
                    layout: left_in,
                },
            )?
        };
        crate::hw::vulkan::acquired_outside(frame, ash::vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
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
        use objc2_metal::{MTLDevice, MTLTextureDescriptor, MTLTextureType, MTLTextureUsage};
        use wgpu::hal::api::Metal;
        #[link(name = "CoreVideo", kind = "framework")]
        unsafe extern "C" {
            fn CVPixelBufferGetIOSurface(buffer: *mut std::ffi::c_void) -> *const IOSurfaceRef;
        }
        if !gpu.metal {
            return Err("the display's device isn't Metal".into());
        }
        let formats = videotoolbox_formats(gpu, picture.layout())?;
        // SAFETY: a VideoToolbox frame's `data[3]` is its pixel buffer, alive as long as the frame.
        let surface = unsafe {
            let buffer = (*picture.frame().as_ptr()).data[3].cast();
            CVPixelBufferGetIOSurface(buffer).as_ref()
        }
        .ok_or("a pixel buffer without an IOSurface")?;
        let planar = surface.plane_count() > 0;
        if surface.plane_count().max(1) != formats.len() {
            return Err(format!("an IOSurface of {} planes", surface.plane_count()));
        }
        // SAFETY: wgpu's own device, which makes textures here only.
        let hal = unsafe { gpu.device.as_hal::<Metal>() }.ok_or("not Metal")?;
        let label = Some("vidcut VideoToolbox picture");
        let plane = |index: usize| -> Result<(wgpu::TextureView, (u32, u32)), String> {
            let (mtl, format) = formats[index];
            let (w, h) = match planar {
                true => (
                    surface.width_of_plane(index),
                    surface.height_of_plane(index),
                ),
                false => (surface.width(), surface.height()),
            };
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
        let mut planes = gpu
            .checked(|| (0..formats.len()).map(plane).collect::<Result<Vec<_>, _>>())??
            .into_iter();
        let (y, size) = planes.next().ok_or("an IOSurface of no planes")?;
        let uv = planes.next().map_or_else(|| y.clone(), |(uv, _)| uv);
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
        let bytes = nvdec_bytes(gpu, layout)?;
        let (w, h) = (u64::from(picture.width()), u64::from(picture.height()));
        let (cw, ch) = chroma_size(layout, picture.width(), picture.height());
        let (cw, ch) = (u64::from(cw), u64::from(ch));
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
        self.shared
            .retain(|s| shared_kept(s.size(), need, s.free()));
        let free = |shared: &[shared::Shared]| {
            shared
                .iter()
                .position(|s| shared_usable(s.size(), need, s.free()))
        };
        let i = match free(&self.shared) {
            Some(i) => i,
            None if self.shared.len() < MAX_SHARED => {
                let made = shared::Shared::new(&gpu.device, cuda, need)?;
                self.shared.push(made);
                self.shared.len() - 1
            }
            None => {
                let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
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
        let alpha = picture.alpha_plane();
        let key = (layout, w, h, wide, alpha);
        if self.planes.as_ref().is_none_or(|p| p.key != key) {
            self.written = None;
            let (one, two, four) = if wide {
                (
                    wgpu::TextureFormat::R16Unorm,
                    wgpu::TextureFormat::Rg16Unorm,
                    wgpu::TextureFormat::Rgba16Unorm,
                )
            } else {
                (
                    wgpu::TextureFormat::R8Unorm,
                    wgpu::TextureFormat::Rg8Unorm,
                    wgpu::TextureFormat::Rgba8Unorm,
                )
            };
            let made = |size: (u32, u32), format, usage| {
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
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_DST
                        | usage,
                    view_formats: &[],
                })
            };
            let make = |size: (u32, u32), format| made(size, format, wgpu::TextureUsages::empty());
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
                    a: make(
                        if alpha { (w, h) } else { (1, 1) },
                        wgpu::TextureFormat::R8Unorm,
                    ),
                },
                Layout::Nv12 | Layout::P010 | Layout::P210 | Layout::P416 => Planes {
                    key,
                    y: make((w, h), one),
                    u: make((cw, ch), two),
                    v: make((1, 1), one),
                    a: make((1, 1), wgpu::TextureFormat::R8Unorm),
                },
                Layout::Ayuv64 => Planes {
                    key,
                    y: make((w, h), four),
                    u: make((1, 1), one),
                    v: make((1, 1), one),
                    a: make((1, 1), wgpu::TextureFormat::R8Unorm),
                },
                Layout::Rgb => Planes {
                    key,
                    y: made(
                        (w, h),
                        wgpu::TextureFormat::Rgba8Unorm,
                        wgpu::TextureUsages::STORAGE_BINDING,
                    ),
                    u: make((1, 1), one),
                    v: make((1, 1), one),
                    a: make((1, 1), wgpu::TextureFormat::R8Unorm),
                },
            });
        }
    }

    fn write_planes(&mut self, gpu: &Gpu, picture: &Picture, encoder: &mut wgpu::CommandEncoder) {
        let (w, h) = (picture.width(), picture.height());
        let layout = picture.layout();
        if layout == Layout::Rgb {
            return self.write_rgb(gpu, picture.frame(), encoder);
        }
        let (cw, ch) = chroma_size(layout, w, h);
        let wide = gpu.wide && sixteen(layout);
        let planes = self.planes.as_ref().expect("made for it");
        let frame = picture.frame();
        crate::copies::up(frame);
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
                if picture.alpha_plane() {
                    write_alpha(gpu, &planes.a, frame, (w, h));
                }
            }
            (Layout::Yuv420p10 | Layout::Yuv422p10 | Layout::Yuv444p12, false) => {
                let shift = if layout == Layout::Yuv444p12 { 4 } else { 2 };
                if picture.alpha_plane() {
                    write_alpha(gpu, &planes.a, frame, (w, h));
                }
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
            (Layout::Rgb, _) => {}
            (Layout::Ayuv64, true) => put(&planes.y, 0, (w, h), 8),
            (Layout::P010 | Layout::P210 | Layout::P416 | Layout::Ayuv64, false) => {
                let textures = match layout {
                    Layout::Ayuv64 => vec![(&planes.y, 0, (w, h), 4)],
                    _ => vec![(&planes.y, 0, (w, h), 1), (&planes.u, 1, (cw, ch), 2)],
                };
                for (texture, plane, size, samples) in textures {
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

    fn write_rgb(
        &mut self,
        gpu: &Gpu,
        frame: &crate::ffmpeg::frame::Video,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let found = held::of(frame);
        let lent = match found {
            Some(h) if h.on(gpu) => Some(h.buffer().clone()),
            Some(h) => match h.imported_on(&gpu.device) {
                Some(Ok(buffer)) => Some(buffer),
                Some(Err(e)) => {
                    held::stop_sharing(&e);
                    None
                }
                None => None,
            },
            None => None,
        };
        let source = match lent {
            Some(buffer) => buffer,
            None => {
                let bytes = match found {
                    Some(_) => held::rgb(frame),
                    None => Ok(crate::buffers::packed(frame, 3)),
                };
                let mut bytes = match bytes {
                    Ok(bytes) => bytes,
                    Err(e) => return eprintln!("vidcut: an RGB picture didn't reach the GPU: {e}"),
                };
                bytes.resize(bytes.len().next_multiple_of(4), 0);
                let size = bytes.len() as u64;
                if self.rgb.as_ref().is_none_or(|b| b.size() != size) {
                    self.rgb = Some(gpu.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("vidcut rgb picture"),
                        size,
                        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }));
                }
                let buffer = self.rgb.clone().expect("made above");
                gpu.queue.write_buffer(&buffer, 0, &bytes);
                crate::copies::up(frame);
                buffer
            }
        };
        let planes = self.planes.as_ref().expect("made for it");
        gpu.held.unpack(&gpu.device, encoder, &source, &planes.y);
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
    pool: &mut D3d11Pool,
    picture: &Arc<Picture>,
    formats: D3d11Formats,
) -> Result<(ash::vk::Image, bool), String> {
    let frame = picture.frame();
    let held = pool
        .iter()
        .find(|(_, s)| {
            std::ptr::eq(s.holds.as_ptr(), Arc::as_ptr(picture)) && !s.busy.load(Ordering::Acquire)
        })
        .map(|(image, _)| *image);
    if let Some(image) = held {
        return Ok((image, false));
    }
    let free = |pool: &D3d11Pool| {
        pool.iter()
            .find(|(_, s)| s.texture.fits(frame) && !s.busy.load(Ordering::Acquire))
            .map(|(image, _)| *image)
    };
    if let Some(image) = free(pool) {
        return Ok((image, false));
    }
    if pool.iter().filter(|(_, s)| s.texture.fits(frame)).count() >= MAX_SHARED {
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        return free(pool)
            .map(|image| (image, false))
            .ok_or_else(|| "no shared texture came free".into());
    }
    if pool.len() >= 2 * MAX_SHARED {
        let oldest = pool
            .iter()
            .find(|(_, s)| !s.busy.load(Ordering::Acquire))
            .map(|(image, _)| *image);
        if let Some(oldest) = oldest {
            pool.remove(&oldest);
        }
    }
    let shared = share_d3d11(gpu, frame, formats)?;
    let image = shared.image;
    pool.insert(image, shared);
    crate::debug!(
        "viewer",
        "imported a {}x{} {:?} Direct3D 11 texture ({} in all)",
        frame.width(),
        frame.height(),
        formats.0,
        pool.len()
    );
    Ok((image, true))
}

#[cfg(windows)]
fn share_d3d11(
    gpu: &Gpu,
    frame: &crate::ffmpeg::frame::Video,
    (format, one, two): D3d11Formats,
) -> Result<D3d11Shared, String> {
    use ::windows::Win32::Foundation::CloseHandle;
    use ::windows::Win32::Graphics::Direct3D11::{D3D11_TEXTURE2D_DESC, ID3D11Texture2D};
    use ::windows::Win32::Graphics::Dxgi::{DXGI_SHARED_RESOURCE_READ, IDXGIResource1};
    use ::windows::core::Interface;
    use wgpu::hal::api::Vulkan;
    let shared = crate::hw::d3d11::Shared::new(frame).map_err(|e| e.to_string())?;
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
        holds: std::sync::Weak::new(),
    })
}

fn nvdec_bytes(gpu: &Gpu, layout: Layout) -> Result<u64, String> {
    match layout {
        Layout::Nv12 => Ok(1),
        Layout::P010 | Layout::P210 if gpu.wide => Ok(2),
        Layout::P010 | Layout::P210 => Err(format!("no 16-bit textures for {layout:?}")),
        Layout::P416 | Layout::Ayuv64 | Layout::Rgb => {
            Err(format!("{layout:?} pictures aren't NVDEC's"))
        }
        Layout::Yuv420p
        | Layout::Yuv420p10
        | Layout::Yuv422p
        | Layout::Yuv422p10
        | Layout::Yuv444p12 => Err("planar pictures aren't NVDEC's".into()),
    }
}

#[cfg(target_os = "linux")]
fn vaapi_formats(
    gpu: &Gpu,
    layout: Layout,
) -> Result<(wgpu::TextureFormat, wgpu::TextureFormat), String> {
    match layout {
        Layout::Nv12 => Ok((wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rg8Unorm)),
        Layout::P010 if gpu.wide => Ok((
            wgpu::TextureFormat::R16Unorm,
            wgpu::TextureFormat::Rg16Unorm,
        )),
        Layout::P010 => Err("no 16-bit textures for P010".into()),
        Layout::P210 | Layout::P416 | Layout::Ayuv64 | Layout::Rgb => {
            Err(format!("{layout:?} pictures aren't VA-API's"))
        }
        Layout::Yuv420p
        | Layout::Yuv420p10
        | Layout::Yuv422p
        | Layout::Yuv422p10
        | Layout::Yuv444p12 => Err("planar pictures aren't VA-API's".into()),
    }
}

#[cfg(windows)]
fn d3d11_formats(
    gpu: &Gpu,
    layout: Layout,
) -> Result<
    (
        wgpu::TextureFormat,
        wgpu::TextureFormat,
        wgpu::TextureFormat,
    ),
    String,
> {
    match layout {
        Layout::Nv12 => Ok((
            wgpu::TextureFormat::NV12,
            wgpu::TextureFormat::R8Unorm,
            wgpu::TextureFormat::Rg8Unorm,
        )),
        Layout::P010 if gpu.wide && gpu.p010 => Ok((
            wgpu::TextureFormat::P010,
            wgpu::TextureFormat::R16Unorm,
            wgpu::TextureFormat::Rg16Unorm,
        )),
        Layout::P010 => Err("no P010 or 16-bit textures for P010".into()),
        Layout::P210 | Layout::P416 | Layout::Ayuv64 | Layout::Rgb => {
            Err(format!("{layout:?} pictures aren't Direct3D 11's"))
        }
        Layout::Yuv420p
        | Layout::Yuv420p10
        | Layout::Yuv422p
        | Layout::Yuv422p10
        | Layout::Yuv444p12 => Err("planar pictures aren't Direct3D 11's".into()),
    }
}

#[cfg(target_os = "macos")]
fn videotoolbox_formats(
    gpu: &Gpu,
    layout: Layout,
) -> Result<&'static [(objc2_metal::MTLPixelFormat, wgpu::TextureFormat)], String> {
    use objc2_metal::MTLPixelFormat;
    use wgpu::TextureFormat as F;
    match layout {
        Layout::Nv12 => Ok(&[
            (MTLPixelFormat::R8Unorm, F::R8Unorm),
            (MTLPixelFormat::RG8Unorm, F::Rg8Unorm),
        ]),
        Layout::P010 | Layout::P210 | Layout::P416 if gpu.wide => Ok(&[
            (MTLPixelFormat::R16Unorm, F::R16Unorm),
            (MTLPixelFormat::RG16Unorm, F::Rg16Unorm),
        ]),
        Layout::Ayuv64 if gpu.wide => Ok(&[(MTLPixelFormat::RGBA16Unorm, F::Rgba16Unorm)]),
        Layout::P010 | Layout::P210 | Layout::P416 | Layout::Ayuv64 => {
            Err(format!("no 16-bit textures for {layout:?}"))
        }
        Layout::Rgb => Err("RGB pictures aren't VideoToolbox's".into()),
        Layout::Yuv420p
        | Layout::Yuv420p10
        | Layout::Yuv422p
        | Layout::Yuv422p10
        | Layout::Yuv444p12 => Err("planar pictures aren't VideoToolbox's".into()),
    }
}

fn vulkan_layout(gpu: &Gpu, layout: Layout) -> Result<(), String> {
    match layout {
        Layout::Yuv420p10 | Layout::Yuv422p10 | Layout::Yuv444p12 if gpu.wide => Ok(()),
        Layout::Yuv420p10 | Layout::Yuv422p10 | Layout::Yuv444p12 => {
            Err("no 16-bit textures for 10- and 12-bit planes".into())
        }
        Layout::Nv12
        | Layout::P010
        | Layout::P210
        | Layout::P416
        | Layout::Ayuv64
        | Layout::Rgb
        | Layout::Yuv420p
        | Layout::Yuv422p => Err(format!("{layout:?} pictures aren't Vulkan's")),
    }
}

#[cfg(any(target_os = "linux", windows))]
fn import_vulkan_frame(
    gpu: &Gpu,
    frame: &crate::ffmpeg::frame::Video,
) -> Result<vulkan::Import, String> {
    use wgpu::hal::api::Vulkan;
    let exported = crate::hw::vulkan::Exported::of(frame).map_err(|e| e.to_string())?;
    if !(3..=4).contains(&exported.planes.len()) {
        return Err(format!("a picture of {} planes", exported.planes.len()));
    }
    let families = exported.planes[0].families.clone();
    let concurrent = families.len() > 1;
    let label = Some("vidcut Vulkan plane");
    // SAFETY: `hal` is wgpu's live device, on the GPU and driver of FFmpeg's (checked first).
    let raws = unsafe {
        let hal = gpu.device.as_hal::<Vulkan>().ok_or("not Vulkan")?;
        if vulkan::uuids(&hal) != (exported.device_uuid, exported.driver_uuid) {
            return Err("FFmpeg's Vulkan device isn't on the display's GPU".into());
        }
        if concurrent && !families.contains(&hal.queue_family_index()) {
            return Err("FFmpeg's planes aren't shared with the display's queue".into());
        }
        let imported = exported
            .planes
            .into_iter()
            .map(|plane| vulkan::import(&hal, plane))
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
    Ok(vulkan::Import {
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

const LEVELS: usize = vidcut_core::render::NESTED + 1;

fn shader() -> String {
    format!(
        "const LEVELS: u32 = {LEVELS}u;\n{}",
        include_str!("picture.wgsl")
    )
}

const PER_LEVEL: usize = 5;

const PARAMS: usize = (15 + PER_LEVEL * LEVELS) * 16;

const LETTERING: usize = (3 + PER_LEVEL * LEVELS) * 16;

fn halvings(
    picture: &Picture,
    size: (u32, u32),
    (placing, kernel, ..): (Option<&(Placing, f64)>, u32, Option<Hdr>, bool),
) -> u32 {
    if kernel & WIDENS == 0 || kernel & 7 == 0 {
        return 0;
    }
    let origin = match picture.sampling() {
        Sampling::Placed { .. } => return 0,
        Sampling::Colors { origin, .. } => (f64::from(origin.0), f64::from(origin.1)),
        Sampling::Own => (0.0, 0.0),
    };
    let (sw, sh) = (f64::from(picture.width()), f64::from(picture.height()));
    let upright = if picture.rotation() % 180 == 90 {
        (sh, sw)
    } else {
        (sw, sh)
    };
    let most = match innermost(placing.map(|(p, _)| p)).last() {
        None => {
            let (_, _, w, h) = picture.placement(size.0, size.1);
            f64::max(upright.0 / f64::from(w), upright.1 / f64::from(h))
        }
        Some(l) => {
            let [a, b, e, c, d, f, g, h] = l.back;
            let (fw, fh) = (f64::from(size.0), f64::from(size.1));
            [(0.0, 0.0), (fw, 0.0), (0.0, fh), (fw, fh)]
                .into_iter()
                .map(|(x, y)| {
                    let (x, y) = (x + origin.0, y + origin.1);
                    let w = g * x + h * y + 1.0;
                    if w <= 0.0 {
                        return f64::INFINITY;
                    }
                    let (u, v) = ((a * x + b * y + e) / w, (c * x + d * y + f) / w);
                    let across = ((a - u * g) / w * upright.0, (c - v * g) / w * upright.1);
                    let down = ((b - u * h) / w * upright.0, (d - v * h) / w * upright.1);
                    f64::max(across.0.hypot(across.1), down.0.hypot(down.1))
                })
                .fold(0.0, f64::max)
        }
    };
    if most.is_nan() || most <= 4.0 {
        return 0;
    }
    let most_halvings = picture.width().max(picture.height()).max(1).ilog2();
    let wanted = (most / 4.0).log2().ceil();
    if wanted.is_finite() {
        (wanted as u32).min(most_halvings)
    } else {
        most_halvings
    }
}

fn innermost(placing: Option<&Placing>) -> &[Level] {
    placing.map_or(&[][..], |p| {
        &p.levels[p.levels.len().saturating_sub(LEVELS)..]
    })
}

fn level_floats(levels: &[Level]) -> Vec<f64> {
    let mut floats = Vec::with_capacity(4 * PER_LEVEL * LEVELS);
    for l in levels {
        let [a, b, e, c, d, f, g, h] = l.back;
        floats.extend([a, b, e, g, c, d, f, h]);
        floats.extend(l.edge);
        floats.extend(l.soft.inner);
        floats.extend([l.soft.reach[0], l.soft.reach[1], l.opacity, 0.0]);
    }
    floats.resize(4 * PER_LEVEL * LEVELS, 0.0);
    floats
}

fn lettering_of(
    map: [f64; 8],
    alpha: f64,
    placing: Option<&Placing>,
    colors: Option<Hdr>,
) -> Vec<u8> {
    let [a, b, e, c, d, f, g, h] = map;
    let levels = innermost(placing);
    let transfer = transfer_of(colors);
    let mut floats = vec![
        a,
        b,
        e,
        alpha,
        c,
        d,
        f,
        levels.len() as f64,
        g,
        h,
        transfer,
        0.0,
    ];
    floats.extend(level_floats(levels));
    floats
        .into_iter()
        .flat_map(|v| (v as f32).to_le_bytes())
        .collect()
}

fn write_alpha(
    gpu: &Gpu,
    texture: &wgpu::Texture,
    frame: &crate::ffmpeg::frame::Video,
    (w, h): (u32, u32),
) {
    let (stride, data) = (frame.stride(3), frame.data(3));
    let row = w as usize;
    let mut top = Vec::with_capacity(row * h as usize);
    for y in 0..h as usize {
        let line = &data[y * stride..y * stride + row * 2];
        top.extend(
            line.as_chunks::<2>()
                .0
                .iter()
                .map(|s| (u16::from_le_bytes(*s) >> 4) as u8),
        );
    }
    write_plane(&gpu.queue, texture, &top, row, (w, h), 1);
}

fn sixteen(layout: Layout) -> bool {
    match layout {
        Layout::Yuv420p | Layout::Nv12 | Layout::Yuv422p | Layout::Rgb => false,
        Layout::P010
        | Layout::Yuv420p10
        | Layout::Yuv422p10
        | Layout::P210
        | Layout::Yuv444p12
        | Layout::P416
        | Layout::Ayuv64 => true,
    }
}

fn same_data(a: &Picture, b: &Picture) -> bool {
    let (a, b) = (a.frame(), b.frame());
    match (held::of(a), held::of(b)) {
        (Some(x), Some(y)) => x.buffer() == y.buffer(),
        (None, None) => {
            a.format() == b.format()
                && (a.width(), a.height()) == (b.width(), b.height())
                && (0..a.planes()).all(|i| a.data(i).as_ptr() == b.data(i).as_ptr())
        }
        _ => false,
    }
}

fn packing(layout: Layout) -> u32 {
    match layout {
        Layout::Yuv420p
        | Layout::Yuv420p10
        | Layout::Yuv422p
        | Layout::Yuv422p10
        | Layout::Yuv444p12 => 0,
        Layout::Nv12 | Layout::P010 | Layout::P210 | Layout::P416 => 1,
        Layout::Ayuv64 => 2,
        Layout::Rgb => 3,
    }
}

fn per_chroma(layout: Layout) -> (u32, u32) {
    match layout {
        Layout::Yuv420p | Layout::Nv12 | Layout::P010 | Layout::Yuv420p10 => (2, 2),
        Layout::Yuv422p | Layout::Yuv422p10 | Layout::P210 => (2, 1),
        Layout::Yuv444p12 | Layout::P416 | Layout::Ayuv64 | Layout::Rgb => (1, 1),
    }
}

fn chroma_site(frame: &crate::ffmpeg::frame::Video, layout: Layout) -> [f64; 2] {
    // SAFETY: a plain field of a live frame.
    let at = unsafe { (*frame.as_ptr()).chroma_location };
    let (mut x, mut y) = (0, 0);
    // SAFETY: two plain out values, which FFmpeg sets only when it knows `at`.
    if unsafe { crate::ffmpeg::ffi::av_chroma_location_enum_to_pos(&mut x, &mut y, at) } < 0 {
        return [0.0; 2];
    }
    let (across, down) = per_chroma(layout);
    let moved = |per: u32, at: i32| {
        if per > 1 {
            0.5 - (0.5 + f64::from(at) / 256.0) / f64::from(per)
        } else {
            0.0
        }
    };
    [moved(across, x), moved(down, y)]
}

fn chroma_size(layout: Layout, w: u32, h: u32) -> (u32, u32) {
    let (across, down) = per_chroma(layout);
    (w.div_ceil(across), h.div_ceil(down))
}

fn shared_kept(size: u64, need: u64, free: bool) -> bool {
    size >= need || !free
}

fn shared_usable(size: u64, need: u64, free: bool) -> bool {
    free && size >= need
}

#[cfg(any(target_os = "linux", test))]
fn reclaim<T>(raws: &mut Vec<T>, mut destroy: impl FnMut(T)) {
    for raw in raws.drain(..) {
        destroy(raw);
    }
}

#[cfg(target_os = "linux")]
struct HeldPlanes<'a> {
    gpu: &'a Gpu,
    raws: Vec<wgpu::hal::vulkan::Texture>,
}

#[cfg(target_os = "linux")]
impl Drop for HeldPlanes<'_> {
    fn drop(&mut self) {
        let Some(hal) = (unsafe { self.gpu.device.as_hal::<wgpu::hal::api::Vulkan>() }) else {
            return;
        };
        reclaim(&mut self.raws, |raw| {
            // SAFETY: each image was made on this device and was not given to wgpu.
            unsafe { wgpu::hal::Device::destroy_texture(&*hal, raw) };
        });
    }
}

fn params_of(
    picture: &Picture,
    size: (u32, u32),
    wide: bool,
    cover: (f64, f64),
    (placing, kernel, timeline, paired): (Option<&(Placing, f64)>, u32, Option<Hdr>, bool),
    halvings: u32,
) -> Vec<u8> {
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
        (Layout::P416 | Layout::Ayuv64, true) => (65535.0, 65535.0),
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
    let g = match timeline {
        Some(_) => crate::color::to_bt2020(picture.colors()),
        None => colors.gamut,
    };
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
    let uv = packing(layout);
    let (across, down) = per_chroma(layout);
    let mixes_in = transfer_of(timeline) as u32;
    let alpha = u32::from(picture.alpha_plane());
    for v in [hdr, turns, uv, kernel, across, down, mixes_in, alpha] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let levels = innermost(placing.map(|(p, _)| p));
    let mut floats = vec![
        levels.len() as f64,
        placing.map_or(0.0, |(_, l)| *l),
        placing.map_or(0.0, |(p, _)| f64::from(p.mode.code())),
        f64::from(u8::from(paired)),
    ];
    let (made, part) = match picture.sampling() {
        Sampling::Own => ([0.0; 4], [0.0; 4]),
        Sampling::Colors { origin, .. } => (
            [1.0, 0.0, 0.0, 0.0],
            [f64::from(origin.0), f64::from(origin.1), 0.0, 0.0],
        ),
        Sampling::Placed { part, .. } => ([2.0, f64::from(size.0), f64::from(size.1), 0.0], *part),
    };
    floats.extend(made);
    floats.extend(part);
    let site = chroma_site(picture.frame(), layout);
    floats.extend([site[0], site[1], 0.0, 0.0]);
    floats.extend([f64::from(halvings), 0.0, 0.0, 0.0]);
    floats.extend(level_floats(levels));
    for v in floats {
        out.extend_from_slice(&(v as f32).to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Kind, Takes, shown_from_gpu};

    #[test]
    fn a_busy_picture_buffer_stays_when_the_next_picture_is_larger() {
        assert!(super::shared_kept(1_000, 4_000, false));
    }

    #[test]
    fn a_free_picture_buffer_that_is_too_small_is_dropped() {
        assert!(!super::shared_kept(1_000, 4_000, true));
    }

    #[test]
    fn a_free_picture_buffer_that_is_too_small_is_not_reused() {
        assert!(!super::shared_usable(1_000, 4_000, true));
    }

    #[test]
    fn planes_left_out_of_the_display_are_reclaimed() {
        let mut left = vec!["y", "uv"];
        let mut freed = Vec::new();
        super::reclaim(&mut left, |plane| freed.push(plane));
        assert_eq!(freed, ["y", "uv"]);
        assert!(left.is_empty());
    }

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
        let module = naga::front::wgsl::parse_str(&super::shader()).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }
}
