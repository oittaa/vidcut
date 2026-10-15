//! The viewer's picture on the GPU (docs/decisions/preview.md §4): a
//! player's decoded YUV picture uploaded as textures at its own size (from
//! memory, or copied on the GPU from NVDEC's frames, [`super::shared`], and
//! from VA-API's, their surfaces imported as DMA-BUFs), and
//! drawn by one shader pass (`picture.wgsl`) into a target the viewer's
//! size: fitted as export fits it (`Picture::placement`), turned upright,
//! scaled in YUV and converted to BT.709 RGB with the export's math
//! (`vidcut_av::color::shader_colors`, HDR tone-mapped as the export does).
//! egui shows the target as an image, and a screenshot reads it back: what
//! the viewer shows, to the pixel.

use std::sync::{Arc, OnceLock};

use vidcut_av::color::shader_colors;
use vidcut_av::cuda::{CudaDevice, PlaneCopy};
use vidcut_av::hw::Kind;
use vidcut_av::player::{Layout, Picture};
use vidcut_av::thumbs::Image;
use vidcut_core::model::Hdr;

/// The GPU and the shader pass the viewer's screen draws with.
pub struct Gpu {
    /// Takes VA-API's imported surfaces and Direct3D 11's textures over to
    /// wgpu's queue, made at the first.
    #[cfg(any(target_os = "linux", windows))]
    acquirer: OnceLock<Result<super::acquire::Acquirer, String>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    /// The pass for a picture over others while a dissolve plays: drawn at
    /// its level, the blend constant, over what the target holds.
    blended: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// 16-bit textures, for samples of more than 8 bits as they are.
    /// Without them those go up as their top 8 bits.
    wide: bool,
    /// The device can share memory with CUDA ([`super::shared`]).
    shares: bool,
    /// The device imports DMA-BUFs, VA-API's surfaces among them (Linux).
    dma_buf: bool,
    /// The device imports Direct3D 11's NV12 textures by their shared
    /// handles (Windows).
    d3d11: bool,
    /// The device has P010 textures, for Direct3D 11's 10-bit ones.
    #[cfg(windows)]
    p010: bool,
    /// The device is Metal's, which makes textures of VideoToolbox's
    /// IOSurfaces (macOS).
    metal: bool,
    /// CUDA's device on this GPU, once a picture in CUDA memory came.
    cuda: OnceLock<Result<CudaDevice, String>>,
}

/// Shared buffers a screen keeps at most ([`Screen::copy_on_gpu`]).
const MAX_SHARED: usize = 4;

/// The format targets are drawn in, which egui shows as it is.
const TARGET: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The device features the viewer uses where the adapter has them: 16-bit
/// textures, DMA-BUFs for VA-API's pictures (Linux), and Direct3D 11's
/// textures with NV12 and P010 for its pictures (Windows).
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
        // The picture at the constant's level, what is under it at the rest.
        let over = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Constant,
            dst_factor: wgpu::BlendFactor::OneMinusConstant,
            operation: wgpu::BlendOperation::Add,
        };
        let blended = pipeline_with(Some(wgpu::BlendState {
            color: over,
            alpha: over,
        }));
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
            // SAFETY: wgpu's own device, only asked what it is.
            #[cfg(target_os = "macos")]
            metal: unsafe { device.as_hal::<wgpu::hal::api::Metal>() }.is_some(),
            #[cfg(not(target_os = "macos"))]
            metal: false,
            cuda: OnceLock::new(),
        }
    }

    /// The decoders whose pictures it may show from GPU memory, so a player
    /// can keep them there for it: NVDEC's where it shares memory with
    /// CUDA, VA-API's where it imports DMA-BUFs, Direct3D 11's where it
    /// imports their textures, VideoToolbox's on Metal. Those it can't take
    /// after all (on another GPU, a surface it won't import) are copied
    /// down to be shown.
    pub fn gpu_pictures(&self) -> Vec<vidcut_av::hw::Kind> {
        use vidcut_av::hw::Kind;
        [
            (Kind::Nvdec, self.shares),
            (Kind::Vaapi, self.dma_buf),
            (Kind::D3d11va, self.d3d11),
            (Kind::VideoToolbox, self.metal),
        ]
        .into_iter()
        .filter_map(|(kind, takes)| takes.then_some(kind))
        .collect()
    }

    /// Takes `images` (just imported, not yet used by wgpu) over from the API
    /// that wrote them, into `layout` for `access` at `stage`, in a command
    /// buffer of its own submitted on wgpu's queue: work submitted after it
    /// finds them so ([`super::acquire::Acquirer`]).
    ///
    /// # Safety
    /// The images are this device's, on memory another API wrote.
    #[cfg(any(target_os = "linux", windows))]
    unsafe fn acquire(
        &self,
        images: &[ash::vk::Image],
        layout: ash::vk::ImageLayout,
        stage: ash::vk::PipelineStageFlags,
        access: ash::vk::AccessFlags,
    ) -> Result<(), String> {
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
        // SAFETY: an encoder of its own, recorded raw and nothing else; the
        // barrier goes into its open command buffer, which wgpu ends. The
        // images are as the caller says.
        let recorded = unsafe {
            encoder.as_hal_mut::<wgpu::hal::api::Vulkan, _, _>(|hal| {
                hal.map(|hal| acquirer.record(hal.raw_handle(), images, layout, stage, access))
            })
        };
        recorded.ok_or("not Vulkan")?;
        self.queue.submit([encoder.finish()]);
        Ok(())
    }

    /// Runs `f`, which makes textures or views, the device's validation
    /// errors in it an error here rather than wgpu's panic (gotchas.md): a
    /// picture the device won't import comes down instead. Nothing recorded
    /// for the GPU goes in it: a command it refused would fail its whole
    /// submission later.
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

/// A picture's planes on the GPU, for one size and layout.
struct Planes {
    key: (Layout, u32, u32, bool),
    y: wgpu::Texture,
    u: wgpu::Texture,
    /// The V plane of planar pictures; a stand-in otherwise.
    v: wgpu::Texture,
}

/// The target a screen draws into, and what it holds.
struct Target {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
}

/// One viewer's picture on the GPU: the target it is drawn into, what that
/// target shows now, and the planes of each picture drawn into it (while a
/// dissolve plays, each of its pictures has its own).
#[derive(Default)]
pub struct Screen {
    target: Option<Target>,
    /// The picture in the target (`None` inside: black).
    drawn: Option<Option<Arc<Picture>>>,
    /// Each layer's: the first the picture's own, then those over it.
    slots: Vec<Slot>,
}

/// One picture's planes on the GPU and how they got there.
#[derive(Default)]
struct Slot {
    planes: Option<Planes>,
    params: Option<wgpu::Buffer>,
    /// Memory CUDA copies pictures into, for the planes: a few, so one is
    /// free while copies out of the others are in flight.
    shared: Vec<super::shared::Shared>,
    /// The one the picture being drawn went through.
    filled: Option<usize>,
    /// Sharing failed: pictures in CUDA memory are copied down from then on.
    unshared: bool,
    /// The layouts whose import failed: pictures in VA-API or Direct3D 11
    /// memory in them are copied down from then on (P010 may fail where
    /// NV12 imports).
    unimported: Vec<Layout>,
    /// The picture being drawn, imported: held until the GPU is done with
    /// its surface, so the surface isn't reused under it.
    imported: Option<Arc<Picture>>,
    /// The luma and chroma planes of the picture being drawn, imported (a
    /// Direct3D 11 texture's): drawn from in place of the planes' textures;
    /// and the texture's size, which is even where the picture's isn't.
    views: Option<(wgpu::TextureView, wgpu::TextureView, (u32, u32))>,
    /// The last picture in GPU memory was copied on the GPU, not down.
    last_shared: bool,
}

/// What [`Screen::draw`] changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drawn {
    /// The target already showed it.
    Same,
    /// The picture changed in the target.
    Picture,
    /// A new target (another size): whoever shows it takes the new one.
    Target,
}

impl Screen {
    pub fn new() -> Self {
        Self::default()
    }

    /// The target's view, once drawn.
    pub fn view(&self) -> Option<&wgpu::TextureView> {
        self.target.as_ref().map(|t| &t.view)
    }

    /// The last picture in GPU memory went into the planes on the GPU, not
    /// copied down.
    pub fn shared(&self) -> bool {
        self.slots.first().is_some_and(|s| s.last_shared)
    }

    /// Draws `picture` (`None`: black) into a `width`×`height` target, as
    /// the viewer shows it.
    pub fn draw(
        &mut self,
        gpu: &Gpu,
        picture: Option<&Arc<Picture>>,
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
            };
            if same {
                return Drawn::Same;
            }
        }
        // A picture larger than the device's textures can't go up (wgpu
        // would panic): black.
        let limit = gpu.device.limits().max_texture_dimension_2d;
        let fits = |p: &Arc<Picture>| p.width() <= limit && p.height() <= limit;
        let picture = picture.filter(|p| fits(p));
        // Bottom up, each with its level over those before (`None`: whole).
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
            for (bind, level) in &binds {
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
            }
        }
        gpu.queue.submit([encoder.finish()]);
        for slot in &mut self.slots[..layers.len()] {
            slot.submitted(gpu);
        }
        self.drawn = Some(picture.cloned());
        drawn
    }
}

impl Slot {
    /// Puts `picture` into its planes (`upload`) and its parameters for a
    /// `size` target into its buffer: the bind group that draws it.
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
        // How much of an imported texture the picture covers.
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
        let v = view(&planes.v);
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

    /// The drawing went to the GPU: the shared buffer it was copied out of
    /// is in use until the GPU is done, and an imported picture is held
    /// until then, so its surface isn't reused under it.
    fn submitted(&mut self, gpu: &Gpu) {
        if let Some(i) = self.filled.take() {
            self.shared[i].used(&gpu.queue);
        }
        if let Some(held) = self.imported.take() {
            gpu.queue.on_submitted_work_done(move || drop(held));
        }
    }

    /// Puts `picture`'s planes into its textures, making them for its size
    /// and layout when they changed: from memory, or for a picture in GPU
    /// memory, copied on the GPU by `encoder` (out of a shared buffer,
    /// `filled` says which, for CUDA's; out of its own surface, imported,
    /// for VA-API's). A picture in GPU memory that can't be shared or
    /// imported is copied down.
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
                // Every shared buffer in flight, or P210: this one comes
                // down.
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

    /// A picture in GPU memory copied down and uploaded.
    fn copy_down(&mut self, gpu: &Gpu, picture: &Picture) {
        match picture.copied_down() {
            Ok(down) => self.write_planes(gpu, &down),
            Err(e) => eprintln!("vidcut: a picture didn't come down from the GPU: {e}"),
        }
    }

    /// Copies a picture in VA-API memory into the planes on the GPU: its
    /// surface exported as DMA-BUFs, each plane imported as a texture of the
    /// device's, and copied by `encoder` into the planes' textures. The
    /// picture is held until those copies are done (`imported`).
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
            Layout::P210 => return Err("P210 pictures aren't VA-API's".into()),
            Layout::Yuv420p | Layout::Yuv420p10 | Layout::Yuv422p | Layout::Yuv422p10 => {
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
            // SAFETY: the file holds the plane, at its offset and pitch,
            // laid out as its modifier says; Vulkan takes the file over.
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
        // wgpu's textures first: one it refuses leaves nothing queued that
        // names its image, and those it makes it keeps until its work on
        // them is done.
        let textures = gpu.checked(|| {
            imports
                .into_iter()
                .map(|(raw, size, format, texture)| {
                    // SAFETY: made on this device, as described, and in the
                    // layout copies read from before wgpu reads it
                    // (acquired below).
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
        // Taken over from VA-API with the picture they hold, ready to copy
        // from.
        // SAFETY: the images of textures just made on wgpu's device, which
        // hasn't used them; only their handles are read.
        unsafe {
            let images: Vec<_> = textures
                .iter()
                .filter_map(|(imported, ..)| imported.as_hal::<Vulkan>().map(|t| t.raw_handle()))
                .collect();
            if images.len() != textures.len() {
                return Err("not Vulkan".into());
            }
            gpu.acquire(
                &images,
                ash::vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                ash::vk::PipelineStageFlags::TRANSFER,
                ash::vk::AccessFlags::TRANSFER_READ,
            )?;
        }
        for (imported, size, texture) in textures {
            // wgpu keeps the texture until the copy is done.
            encoder.copy_texture_to_texture(
                imported.as_image_copy(),
                texture.as_image_copy(),
                size,
            );
        }
        self.imported = Some(picture.clone());
        Ok(())
    }

    /// Shows a picture in Direct3D 11 memory from there: its texture (one of
    /// its own, the engine's copy out of the decoder's, done) opened by
    /// wgpu's Vulkan device through an NT handle, and its planes' views drawn
    /// from in place of the planes' textures (`views`). The picture is held
    /// until the drawing is done (`imported`).
    #[cfg(windows)]
    fn import_d3d11(&mut self, gpu: &Gpu, picture: &Arc<Picture>) -> Result<(), String> {
        use ::windows::Win32::Foundation::CloseHandle;
        use ::windows::Win32::Graphics::Direct3D11::{D3D11_TEXTURE2D_DESC, ID3D11Texture2D};
        use ::windows::Win32::Graphics::Dxgi::{DXGI_SHARED_RESOURCE_READ, IDXGIResource1};
        use ::windows::core::Interface;
        use wgpu::hal::api::Vulkan;
        if !gpu.d3d11 {
            return Err("the display's device imports no Direct3D 11 textures".into());
        }
        let (format, one, two) = match picture.layout() {
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
            Layout::P210 => return Err("P210 pictures aren't Direct3D 11's".into()),
            Layout::Yuv420p | Layout::Yuv420p10 | Layout::Yuv422p | Layout::Yuv422p10 => {
                return Err("planar pictures aren't Direct3D 11's".into());
            }
        };
        // SAFETY: a Direct3D 11 frame holds its texture and its index in it.
        let (texture, index) = unsafe {
            let f = picture.frame().as_ptr();
            ((*f).data[0] as *mut std::ffi::c_void, (*f).data[1] as usize)
        };
        if index != 0 {
            return Err("a picture in a texture array".into());
        }
        // SAFETY: the frame's texture, alive as long as the frame.
        let texture = unsafe { ID3D11Texture2D::from_raw_borrowed(&texture) }
            .ok_or("a picture without a texture")?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: fills the description it is given.
        unsafe { texture.GetDesc(&mut desc) };
        let size = wgpu::Extent3d {
            width: desc.Width,
            height: desc.Height,
            depth_or_array_layers: 1,
        };
        let resource: IDXGIResource1 = texture.cast().map_err(|e| e.to_string())?;
        // SAFETY: a texture made shareable by NT handles (the engine's).
        let handle =
            unsafe { resource.CreateSharedHandle(None, DXGI_SHARED_RESOURCE_READ.0, None) }
                .map_err(|e| format!("sharing a Direct3D 11 texture: {e}"))?;
        let label = Some("vidcut Direct3D 11 picture");
        // SAFETY: the handle is the texture's, as described; Vulkan holds
        // the texture once it is imported, and the handle is closed after.
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
        // wgpu's texture first, as for VA-API's.
        let (image, y, uv) = gpu.checked(|| {
            // SAFETY: made on this device, as described, and in the layout
            // shaders sample before wgpu reads it (acquired below).
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
        // Taken over from Direct3D 11 with the picture it holds, ready to
        // sample.
        // SAFETY: the image of a texture just made on wgpu's device, which
        // hasn't used it.
        unsafe {
            gpu.acquire(
                &[image.ok_or("not Vulkan")?],
                ash::vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                ash::vk::PipelineStageFlags::FRAGMENT_SHADER,
                ash::vk::AccessFlags::SHADER_READ,
            )?;
        }
        self.views = Some((y, uv, (desc.Width, desc.Height)));
        self.imported = Some(picture.clone());
        Ok(())
    }

    /// Shows a picture in VideoToolbox memory from there (macOS): each plane
    /// of its pixel buffer's IOSurface made a Metal texture of wgpu's
    /// device, whose view is drawn from in place of the planes' textures
    /// (`views`). Nothing is copied. The picture is held until the drawing
    /// is done (`imported`), so VideoToolbox doesn't reuse its surface.
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
            /// The IOSurface backing a pixel buffer, or null.
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
            Layout::P010 | Layout::P210 if gpu.wide => [
                (MTLPixelFormat::R16Unorm, F::R16Unorm),
                (MTLPixelFormat::RG16Unorm, F::Rg16Unorm),
            ],
            Layout::P010 => return Err("no 16-bit textures for P010".into()),
            Layout::P210 => return Err("no 16-bit textures for P210".into()),
            Layout::Yuv420p | Layout::Yuv420p10 | Layout::Yuv422p | Layout::Yuv422p10 => {
                return Err("planar pictures aren't VideoToolbox's".into());
            }
        };
        // SAFETY: a VideoToolbox frame holds its pixel buffer in `data[3]`,
        // alive as long as the frame, and the buffer its IOSurface.
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
            // SAFETY: a texture of the plane in its own size and layout;
            // Metal checks the descriptor against the surface.
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
            // SAFETY: a texture of this device, as described, which only
            // the shader reads.
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
        Ok(())
    }

    /// Copies a picture in CUDA memory into the planes on the GPU: CUDA
    /// into a free shared buffer, then `encoder` into the textures. False
    /// when none is free (copies out of all are in flight), and for P210
    /// (NVDEC's 4:2:2), which no GPU here decodes to test a copy of.
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
            Layout::Yuv420p | Layout::Yuv420p10 | Layout::Yuv422p | Layout::Yuv422p10 => {
                return Err("planar pictures aren't NVDEC's".into());
            }
        };
        let (w, h) = (u64::from(picture.width()), u64::from(picture.height()));
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        // Rows and planes where a copy into a texture may start.
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
        // Too small for these pictures: let go (wgpu keeps what a copy in
        // flight reads until it is done).
        self.shared.retain(|s| s.size() >= need);
        let free = |shared: &[super::shared::Shared]| shared.iter().position(|s| s.free());
        let i = match free(&self.shared) {
            Some(i) => i,
            // All in flight: another (a display refreshing at most a few
            // pictures ahead never needs many).
            None if self.shared.len() < MAX_SHARED => {
                let made = super::shared::Shared::new(&gpu.device, cuda, need)?;
                self.shared.push(made);
                self.shared.len() - 1
            }
            None => {
                // Work done but not noticed yet: notice it, without waiting.
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

    /// Makes the planes' textures for `picture`'s size and layout, when
    /// they changed.
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
                Layout::Yuv420p | Layout::Yuv420p10 | Layout::Yuv422p | Layout::Yuv422p10 => {
                    Planes {
                        key,
                        y: make((w, h), one),
                        u: make((cw, ch), one),
                        v: make((cw, ch), one),
                    }
                }
                Layout::Nv12 | Layout::P010 | Layout::P210 => Planes {
                    key,
                    y: make((w, h), one),
                    u: make((cw, ch), two),
                    v: make((1, 1), one),
                },
            });
        }
    }

    /// Uploads `picture`'s planes from memory into the planes made for it.
    fn write_planes(&self, gpu: &Gpu, picture: &Picture) {
        let (w, h) = (picture.width(), picture.height());
        let layout = picture.layout();
        let (cw, ch) = chroma_size(layout, w, h);
        let wide = gpu.wide && sixteen(layout);
        let planes = self.planes.as_ref().expect("made for it");
        let frame = picture.frame();
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
            (Layout::Yuv420p10 | Layout::Yuv422p10, true) => {
                put(&planes.y, 0, (w, h), 2);
                put(&planes.u, 1, (cw, ch), 2);
                put(&planes.v, 2, (cw, ch), 2);
            }
            // Without 16-bit textures: the top 8 of each sample's 10 bits,
            // which are the low bits of 16.
            (Layout::Yuv420p10 | Layout::Yuv422p10, false) => {
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
                                .map(|s| (u16::from_le_bytes(*s) >> 2) as u8),
                        );
                    }
                    write_plane(&gpu.queue, texture, &top, row, size, 1);
                }
            }
            (Layout::Nv12, _) => {
                put(&planes.y, 0, (w, h), 1);
                put(&planes.u, 1, (cw, ch), 2);
            }
            (Layout::P010 | Layout::P210, true) => {
                put(&planes.y, 0, (w, h), 2);
                put(&planes.u, 1, (cw, ch), 4);
            }
            // Without 16-bit textures: the high byte of each sample.
            (Layout::P010 | Layout::P210, false) => {
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
    /// The target's pixels as RGBA rows, top first: what the viewer shows.
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
        let submission = gpu.queue.submit([encoder.finish()]);
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

/// Copies a plane's rows (`stride` bytes apart, `bytes` per sample) into
/// `texture`.
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

/// The size of the shader's parameters: nine vectors of four.
const PARAMS: usize = 9 * 16;

/// Its samples take 16 bits.
fn sixteen(layout: Layout) -> bool {
    match layout {
        Layout::Yuv420p | Layout::Nv12 | Layout::Yuv422p => false,
        Layout::P010 | Layout::Yuv420p10 | Layout::Yuv422p10 | Layout::P210 => true,
    }
}

/// Its U and V are interleaved in one plane.
fn interleaved(layout: Layout) -> bool {
    match layout {
        Layout::Nv12 | Layout::P010 | Layout::P210 => true,
        Layout::Yuv420p | Layout::Yuv420p10 | Layout::Yuv422p | Layout::Yuv422p10 => false,
    }
}

/// Luma rows per chroma row: 2 for 4:2:0, 1 for 4:2:2.
fn rows_per_chroma_row(layout: Layout) -> u32 {
    match layout {
        Layout::Yuv420p | Layout::Nv12 | Layout::P010 | Layout::Yuv420p10 => 2,
        Layout::Yuv422p | Layout::Yuv422p10 | Layout::P210 => 1,
    }
}

/// The size of its chroma planes in a `w`×`h` picture: half as wide, and
/// half as tall for 4:2:0.
fn chroma_size(layout: Layout, w: u32, h: u32) -> (u32, u32) {
    (w.div_ceil(2), h.div_ceil(rows_per_chroma_row(layout)))
}

/// The shader's parameters for `picture` in a `size` target. `wide`: samples
/// of more than 8 bits go up in 16. `cover`: how much of its texture's width
/// and height the stored picture covers (1 but for a texture rounded up to
/// even sides).
fn params_of(picture: &Picture, size: (u32, u32), wide: bool, cover: (f64, f64)) -> Vec<u8> {
    let colors = shader_colors(picture.colors());
    let (x, y, w, h) = picture.placement(size.0, size.1);
    let turns = u32::from(picture.rotation() / 90) % 4;
    let (sw, sh) = (picture.width(), picture.height());
    let upright = if turns % 2 == 1 { (sh, sw) } else { (sw, sh) };
    // Sample s (0..1) as a code: 8 bits (also the top 8 of 10 without
    // 16-bit textures), or 10 bits in the high bits of 16 (P010, P210) or
    // in the low bits (planar).
    let layout = picture.layout();
    let (code, max): (f64, f64) = match (layout, wide && sixteen(layout)) {
        (Layout::P010 | Layout::P210, true) => (65535.0 / 64.0, 1023.0),
        (Layout::Yuv420p10 | Layout::Yuv422p10, true) => (65535.0, 1023.0),
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
    for v in [hdr, turns, uv, rows_per_chroma_row(layout)] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_shader_compiles() {
        // naga's own check of the WGSL, no GPU needed.
        let module = naga::front::wgsl::parse_str(include_str!("picture.wgsl")).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }
}
