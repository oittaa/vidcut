use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use vidcut_core::model::Hdr;

use super::shared::Shared;
use super::{Gpu, Screen, begin, transfer_of};
use crate::cuda::{CudaDevice, CudaFrames, PlaneCopy};
use crate::error::{Error, Result};
use crate::ffmpeg::{format, frame};

const FORMATS: [wgpu::TextureFormat; 5] = [
    wgpu::TextureFormat::R8Uint,
    wgpu::TextureFormat::Rg8Uint,
    wgpu::TextureFormat::R16Uint,
    wgpu::TextureFormat::Rg16Uint,
    wgpu::TextureFormat::Rgba8Uint,
];

fn unorm(format: wgpu::TextureFormat) -> wgpu::TextureFormat {
    use wgpu::TextureFormat as T;
    match format {
        T::R8Uint => T::R8Unorm,
        T::Rg8Uint => T::Rg8Unorm,
        T::R16Uint => T::R16Unorm,
        T::Rg16Uint => T::Rg16Unorm,
        T::Rgba8Uint => T::Rgba8Unorm,
        other => other,
    }
}

fn top_of(format: wgpu::TextureFormat) -> u32 {
    use wgpu::TextureFormat as T;
    match format {
        T::R16Uint | T::Rg16Uint => 65535,
        _ => 255,
    }
}

const LUMA: u32 = 0;
const PAIR: u32 = 1;
const BLUE: u32 = 2;
const RED: u32 = 3;
const ALPHA: u32 = 4;
const RGBA: u32 = 5;

pub(super) struct Encoders {
    layout: wgpu::BindGroupLayout,
    pipelines: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
}

impl Encoders {
    pub(super) fn new(device: &wgpu::Device, shader: &wgpu::ShaderModule) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vidcut encoding"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 9,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 10,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vidcut encoding"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let uints = FORMATS.iter().map(|&f| (f, "fs_encode"));
        let unorms = FORMATS.iter().map(|&f| (unorm(f), "fs_encode_unorm"));
        let pipelines = uints
            .chain(unorms)
            .filter_map(|(format, entry)| {
                let made = super::checked(device, || {
                    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some("vidcut encoding"),
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
                                format,
                                blend: None,
                                write_mask: wgpu::ColorWrites::ALL,
                            })],
                        }),
                        multiview_mask: None,
                        cache: None,
                    })
                });
                made.ok().map(|pipeline| (format, pipeline))
            })
            .collect();
        Self { layout, pipelines }
    }

    fn pipeline(&self, format: wgpu::TextureFormat) -> Result<&wgpu::RenderPipeline> {
        self.pipelines
            .iter()
            .find(|(f, _)| *f == format)
            .map(|(_, p)| p)
            .ok_or_else(|| Error::Invalid(format!("the GPU can't write {format:?}")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Plane {
    kind: u32,
    format: wgpu::TextureFormat,
    texel: u32,
    sub: (u32, u32),
    packed: bool,
}

struct Layout {
    depth: u32,
    shift: u32,
    planes: Vec<Plane>,
}

fn layout_of(pixel: format::Pixel) -> Option<Layout> {
    use format::Pixel as P;
    use wgpu::TextureFormat as T;
    let plane = |kind, format, texel, sub| Plane {
        kind,
        format,
        texel,
        sub,
        packed: false,
    };
    let planar = |depth: u32, sub: (u32, u32), alpha: bool| {
        let (format, texel) = if depth > 8 {
            (T::R16Uint, 2)
        } else {
            (T::R8Uint, 1)
        };
        let mut planes = vec![
            plane(LUMA, format, texel, (0, 0)),
            plane(BLUE, format, texel, sub),
            plane(RED, format, texel, sub),
        ];
        if alpha {
            planes.push(plane(ALPHA, format, texel, (0, 0)));
        }
        Layout {
            depth,
            shift: 0,
            planes,
        }
    };
    let paired = |depth: u32, shift: u32, sub: (u32, u32)| {
        let (luma, pair, texel) = if depth + shift > 8 {
            (T::R16Uint, T::Rg16Uint, 2)
        } else {
            (T::R8Uint, T::Rg8Uint, 1)
        };
        Layout {
            depth,
            shift,
            planes: vec![
                plane(LUMA, luma, texel, (0, 0)),
                plane(PAIR, pair, texel * 2, sub),
            ],
        }
    };
    Some(match pixel {
        P::YUV420P => planar(8, (1, 1), false),
        P::YUV422P => planar(8, (1, 0), false),
        P::YUV444P => planar(8, (0, 0), false),
        P::YUV420P10LE => planar(10, (1, 1), false),
        P::YUV422P10LE => planar(10, (1, 0), false),
        P::YUV444P10LE => planar(10, (0, 0), false),
        P::YUVA444P10LE => planar(10, (0, 0), true),
        P::YUV420P12LE => planar(12, (1, 1), false),
        P::YUV422P12LE => planar(12, (1, 0), false),
        P::YUV444P12LE => planar(12, (0, 0), false),
        P::YUVA444P12LE => planar(12, (0, 0), true),
        P::NV12 => paired(8, 0, (1, 1)),
        P::NV16 => paired(8, 0, (1, 0)),
        P::NV24 => paired(8, 0, (0, 0)),
        P::P010LE => paired(10, 6, (1, 1)),
        P::P210LE => paired(10, 6, (1, 0)),
        P::P410LE => paired(10, 6, (0, 0)),
        P::P016LE => paired(16, 0, (1, 1)),
        P::P216LE => paired(16, 0, (1, 0)),
        P::P416LE => paired(16, 0, (0, 0)),
        P::RGBA => Layout {
            depth: 8,
            shift: 0,
            planes: vec![plane(RGBA, T::Rgba8Uint, 4, (0, 0))],
        },
        P::RGB24 => Layout {
            depth: 8,
            shift: 0,
            planes: vec![Plane {
                packed: true,
                ..plane(RGBA, T::Rgba8Uint, 4, (0, 0))
            }],
        },
        _ => return None,
    })
}

fn plane_size(p: &Plane, (w, h): (u32, u32)) -> (u32, u32) {
    let sub = |n: u32, s: u32| (n + (1 << s) - 1) >> s;
    (sub(w, p.sub.0), sub(h, p.sub.1))
}

fn params(layout: &Layout, plane: &Plane, (from, to): (Option<Hdr>, Option<Hdr>)) -> Vec<u8> {
    let colors = crate::color::shader_colors(crate::color::Colors {
        matrix: crate::color::Matrix::Bt2020,
        full_range: false,
        hdr: from,
    });
    let g = crate::color::output_gamut(from, to);
    let (kr, kb) = match to {
        Some(_) => (0.2627, 0.0593),
        None => (0.2126, 0.0722),
    };
    let floats: [[f64; 4]; 5] = [
        colors.tone,
        [g[0][0], g[0][1], g[0][2], 0.0],
        [g[1][0], g[1][1], g[1][2], 0.0],
        [g[2][0], g[2][1], g[2][2], 0.0],
        [kr, kb, colors.linear_scale, 0.0],
    ];
    let site = output_site(plane);
    let words: [[u32; 4]; 2] = [
        [
            transfer_of(from) as u32,
            transfer_of(to) as u32,
            layout.depth,
            layout.shift,
        ],
        [plane.kind, plane.sub.0, plane.sub.1, top_of(plane.format)],
    ];
    floats
        .iter()
        .flatten()
        .flat_map(|v| (*v as f32).to_le_bytes())
        .chain(words.iter().flatten().flat_map(|v| v.to_le_bytes()))
        .chain(site.iter().flat_map(|v| (*v as f32).to_le_bytes()))
        .collect()
}

fn output_site(plane: &Plane) -> [f64; 4] {
    use crate::ffmpeg::ffi::AVChromaLocation as L;
    let at = match plane.sub {
        (1, 1) => L::AVCHROMA_LOC_LEFT,
        (1, 0) => L::AVCHROMA_LOC_TOPLEFT,
        _ => return [0.0; 4],
    };
    let (mut x, mut y) = (0, 0);
    // SAFETY: two plain out values, which FFmpeg sets for a known location.
    if unsafe { crate::ffmpeg::ffi::av_chroma_location_enum_to_pos(&mut x, &mut y, at) } < 0 {
        return [0.0; 4];
    }
    let along = |sub: u32, at: i32| if sub > 0 { f64::from(at) / 256.0 } else { 0.0 };
    [along(plane.sub.0, x), along(plane.sub.1, y), 0.0, 0.0]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Memory,
    Cuda,
    Surface,
    Held,
}

struct Made {
    texture: Option<wgpu::Texture>,
    buffer: Option<wgpu::Buffer>,
    packed: Option<wgpu::Buffer>,
    params: wgpu::Buffer,
    size: (u32, u32),
    padded: u32,
    row: u32,
    offset: u64,
}

type Key = (format::Pixel, (u32, u32), Option<Hdr>, Option<Hdr>, Target);

const SLOTS: usize = 2;

#[derive(Default)]
pub(super) struct Encoded {
    key: Option<Key>,
    made: Vec<Made>,
    slots: Vec<Slot>,
    next: usize,
    serial: u64,
}

type Slot = Arc<Mutex<Filling>>;

struct Filling {
    shared: Shared,
    work: Option<Work>,
}

struct Work {
    device: wgpu::Device,
    submission: wgpu::SubmissionIndex,
    planes: Vec<PlaneCopy>,
    into: frame::Video,
    serial: u64,
}

struct Handed {
    slot: Slot,
    serial: u64,
}

static HANDED: u8 = 0;

fn lock(slot: &Slot) -> MutexGuard<'_, Filling> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Filling {
    fn finish(&mut self, serial: Option<u64>) -> Result<(Duration, Duration)> {
        let Some(work) = &self.work else {
            return Ok(Default::default());
        };
        if serial.is_some_and(|s| s != work.serial) {
            return Ok(Default::default());
        }
        let clock = Instant::now();
        wait(&work.device, work.submission.clone())?;
        let waited = clock.elapsed();
        let clock = Instant::now();
        self.shared
            .empty_into(&work.into, &work.planes)
            .map_err(Error::Invalid)?;
        self.work = None;
        Ok((waited, clock.elapsed()))
    }
}

pub(crate) fn filled(f: &mut frame::Video) -> Result<(Duration, Duration)> {
    let Some(handed) = super::carried::carried::<Handed>(f, &HANDED) else {
        return Ok(Default::default());
    };
    let spent = lock(&handed.slot).finish(Some(handed.serial))?;
    super::carried::let_go_of(f);
    Ok(spent)
}

pub enum Sink<'a> {
    Memory,
    Cuda(&'a CudaDevice, &'a CudaFrames),
    Surface(frame::Video),
    Held,
}

impl Sink<'_> {
    fn target(&self) -> Target {
        match self {
            Sink::Memory => Target::Memory,
            Sink::Cuda(..) => Target::Cuda,
            Sink::Surface(_) => Target::Surface,
            Sink::Held => Target::Held,
        }
    }
}

impl Screen {
    pub fn for_export() -> Self {
        Self {
            exported: true,
            ..Self::default()
        }
    }

    pub(crate) fn spent(&self) -> (Duration, Duration) {
        self.spent
    }

    pub fn encoded(
        &mut self,
        gpu: &Gpu,
        pixel: format::Pixel,
        colors: (Option<Hdr>, Option<Hdr>),
        sink: Sink<'_>,
    ) -> Result<frame::Video> {
        let wide = self
            .wide
            .as_ref()
            .ok_or_else(|| Error::Invalid("nothing was drawn for the encoder".into()))?;
        let size = wide.size;
        let layout = layout_of(pixel)
            .ok_or_else(|| Error::Invalid(format!("the GPU can't make frames in {pixel:?}")))?;
        let target = sink.target();
        if target == Target::Held && pixel != format::Pixel::RGB24 {
            return Err(Error::Invalid(format!(
                "the GPU holds RGB24 pictures, not {pixel:?}"
            )));
        }
        let key = Some((pixel, size, colors.0, colors.1, target));
        if self.encoded.key != key {
            self.encoded.key = None;
            self.encoded.slots.clear();
            let mut offset = 0;
            let mut made_all = Vec::new();
            for p in &layout.planes {
                let m = made(gpu, &layout, p, size, colors, offset, target);
                offset = (m.offset + u64::from(m.padded) * u64::from(m.size.1))
                    .next_multiple_of(u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT));
                made_all.push(m);
            }
            self.encoded.made = made_all;
            if let Sink::Cuda(device, _) = &sink {
                for _ in 0..SLOTS {
                    let shared = shared_out(gpu, device, offset)?;
                    let filling = Filling { shared, work: None };
                    self.encoded.slots.push(Arc::new(Mutex::new(filling)));
                }
                self.encoded.next = 0;
            }
            self.encoded.key = key;
        }
        let clock = Instant::now();
        let slot = match &sink {
            Sink::Cuda(..) => {
                let slot = self
                    .encoded
                    .slots
                    .get(self.encoded.next)
                    .cloned()
                    .ok_or_else(|| Error::Invalid("no memory is shared with CUDA".into()))?;
                self.encoded.next = (self.encoded.next + 1) % self.encoded.slots.len();
                let mut filling = lock(&slot);
                filling.finish(None)?;
                let buffer = filling.shared.buffer().clone();
                drop(filling);
                Some((slot, buffer))
            }
            _ => None,
        };
        self.spent.1 += clock.elapsed();
        let surfaces = match &sink {
            Sink::Surface(f) => surface_textures(gpu, f, &layout.planes)?,
            _ => Vec::new(),
        };
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("vidcut encoding"),
            });
        for (i, (plane, m)) in layout.planes.iter().zip(&self.encoded.made).enumerate() {
            let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("vidcut encoding"),
                layout: &gpu.encoders.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: wgpu::BindingResource::TextureView(&wide.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: m.params.as_entire_binding(),
                    },
                ],
            });
            let (texture, format) = match (surfaces.get(i), &m.texture) {
                (Some(t), _) => (t, unorm(plane.format)),
                (None, Some(t)) => (t, plane.format),
                (None, None) => return Err(Error::Invalid("a plane has nowhere to go".into())),
            };
            let pipeline = gpu.encoders.pipeline(format)?;
            let view = texture.create_view(&Default::default());
            {
                let mut pass = begin(&mut encoder, &view, Some(wgpu::Color::TRANSPARENT));
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.draw(0..3, 0..1);
            }
            if let (Some(packed), Some(buffer)) = (&m.packed, &m.buffer) {
                gpu.held
                    .pack_into(&gpu.device, &mut encoder, texture, packed);
                encoder.copy_buffer_to_buffer(packed, 0, buffer, 0, packed.size());
                continue;
            }
            let (buffer, offset) = match (&slot, &m.buffer) {
                (Some((_, shared)), _) => (shared, m.offset),
                (None, Some(buffer)) => (buffer, 0),
                (None, None) => continue,
            };
            encoder.copy_texture_to_buffer(
                texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset,
                        bytes_per_row: Some(m.padded),
                        rows_per_image: Some(m.size.1),
                    },
                },
                wgpu::Extent3d {
                    width: m.size.0,
                    height: m.size.1,
                    depth_or_array_layers: 1,
                },
            );
        }
        let held = match (&sink, self.encoded.made.first()) {
            (
                Sink::Held,
                Some(Made {
                    texture: Some(t), ..
                }),
            ) => Some(gpu.held.pack(&gpu.device, &mut encoder, t)),
            _ => None,
        };
        let submission = gpu.submit([encoder.finish()]);
        let mut out = match sink {
            Sink::Cuda(_, frames) => {
                let (slot, _) =
                    slot.ok_or_else(|| Error::Invalid("no memory is shared with CUDA".into()))?;
                self.handed(gpu, frames, submission, slot)?
            }
            Sink::Memory => self.read_back(gpu, pixel, size, submission)?,
            Sink::Surface(f) => {
                wait(&gpu.device, submission)?;
                f
            }
            Sink::Held => {
                let buffer = held.ok_or_else(|| Error::Invalid("no RGB plane to hold".into()))?;
                let held = super::held::Held::new(buffer, (&gpu.device, &gpu.queue), gpu.id);
                return super::held::picture(held, size, None);
            }
        };
        crate::color::tag_frame(&mut out, colors.1);
        Ok(out)
    }

    fn handed(
        &mut self,
        gpu: &Gpu,
        frames: &CudaFrames,
        submission: wgpu::SubmissionIndex,
        slot: Slot,
    ) -> Result<frame::Video> {
        let mut out = frames.fresh()?;
        let planes: Vec<PlaneCopy> = self
            .encoded
            .made
            .iter()
            .map(|m| PlaneCopy {
                offset: m.offset,
                pitch: u64::from(m.padded),
                row_bytes: u64::from(m.row),
                rows: u64::from(m.size.1),
            })
            .collect();
        let into = crate::export::video::frame_ref(&out)?;
        self.encoded.serial += 1;
        let serial = self.encoded.serial;
        lock(&slot).work = Some(Work {
            device: gpu.device.clone(),
            submission,
            planes,
            into,
            serial,
        });
        super::carried::carry(&mut out, Handed { slot, serial }, &HANDED)?;
        Ok(out)
    }

    fn read_back(
        &self,
        gpu: &Gpu,
        pixel: format::Pixel,
        size: (u32, u32),
        submission: wgpu::SubmissionIndex,
    ) -> Result<frame::Video> {
        let buffers: Vec<&wgpu::Buffer> = self
            .encoded
            .made
            .iter()
            .filter_map(|m| m.buffer.as_ref())
            .collect();
        let (tx, rx) =
            std::sync::mpsc::channel::<std::result::Result<(), wgpu::BufferAsyncError>>();
        for buffer in &buffers {
            let tx = tx.clone();
            buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        }
        drop(tx);
        wait(&gpu.device, submission)?;
        for r in rx {
            r.map_err(|e| Error::Invalid(format!("the GPU gave no frame back: {e}")))?;
        }
        let mut out = frame::Video::new(pixel, size.0, size.1);
        for (i, (m, buffer)) in self.encoded.made.iter().zip(&buffers).enumerate() {
            let stride = out.stride(i);
            let rows = out.data_mut(i);
            {
                let mapped = buffer
                    .slice(..)
                    .get_mapped_range()
                    .map_err(|e| Error::Invalid(format!("the GPU's frame can't be read: {e}")))?;
                let n = (m.row as usize).min(stride);
                for y in 0..m.size.1 as usize {
                    let from = y * m.padded as usize;
                    let to = y * stride;
                    rows[to..to + n].copy_from_slice(&mapped[from..from + n]);
                }
            }
            buffer.unmap();
        }
        crate::copies::down(&out);
        Ok(out)
    }
}

fn wait(device: &wgpu::Device, submission: wgpu::SubmissionIndex) -> Result<()> {
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: None,
        })
        .map(|_| ())
        .map_err(|e| Error::Invalid(format!("the GPU did not finish a frame: {e}")))
}

fn shared_out(gpu: &Gpu, device: &CudaDevice, size: u64) -> Result<Shared> {
    if !super::shared::possible(&gpu.device) {
        return Err(Error::Invalid(
            "the GPU can't share memory with CUDA".into(),
        ));
    }
    let same = device
        .uuid()
        .is_ok_and(|u| super::shared::uuid(&gpu.device) == Some(u));
    if !same {
        return Err(Error::Invalid("CUDA's GPU isn't the drawing's GPU".into()));
    }
    Shared::out(&gpu.device, device, size).map_err(Error::Invalid)
}

#[cfg(target_os = "macos")]
fn surface_textures(gpu: &Gpu, f: &frame::Video, planes: &[Plane]) -> Result<Vec<wgpu::Texture>> {
    use objc2_metal::{MTLPixelFormat, MTLTextureType, MTLTextureUsage};
    use wgpu::TextureFormat as F;
    use wgpu::hal::api::Metal;
    const WHAT: &str = "handing a frame to the encoder";
    if !gpu.metal {
        return Err(Error::Invalid("the GPU isn't Metal".into()));
    }
    let surface = crate::metal::surface(f, WHAT)?;
    // SAFETY: wgpu's own device, which makes textures here only.
    let hal = unsafe { gpu.device.as_hal::<Metal>() }
        .ok_or_else(|| Error::Invalid("the GPU isn't Metal".into()))?;
    let made = |index: usize, plane: &Plane| -> Result<wgpu::Texture> {
        let format = unorm(plane.format);
        let mtl = match format {
            F::R8Unorm => MTLPixelFormat::R8Unorm,
            F::Rg8Unorm => MTLPixelFormat::RG8Unorm,
            F::R16Unorm => MTLPixelFormat::R16Unorm,
            F::Rg16Unorm => MTLPixelFormat::RG16Unorm,
            _ => return Err(Error::Invalid(format!("an IOSurface plane in {format:?}"))),
        };
        let raw = crate::metal::texture(
            hal.raw_device(),
            &surface,
            (index, mtl),
            MTLTextureUsage::RenderTarget,
            WHAT,
        )?;
        let (width, height) = (
            surface.width_of_plane(index) as u32,
            surface.height_of_plane(index) as u32,
        );
        let extent = wgpu::hal::CopyExtent {
            width,
            height,
            depth: 1,
        };
        // SAFETY: `raw` is a texture of this device, as described, which only render passes write.
        Ok(unsafe {
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
                    label: Some("vidcut encoder's picture"),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                },
                wgpu::TextureUses::COLOR_TARGET,
            )
        })
    };
    if surface.plane_count() != planes.len() {
        return Err(Error::Invalid(format!(
            "an IOSurface of {} planes",
            surface.plane_count()
        )));
    }
    gpu.checked(|| {
        planes
            .iter()
            .enumerate()
            .map(|(i, p)| made(i, p))
            .collect::<Result<Vec<_>>>()
    })
    .map_err(Error::Invalid)?
}

#[cfg(not(target_os = "macos"))]
fn surface_textures(_: &Gpu, _: &frame::Video, _: &[Plane]) -> Result<Vec<wgpu::Texture>> {
    Err(Error::Invalid("IOSurfaces need macOS".into()))
}

fn made(
    gpu: &Gpu,
    layout: &Layout,
    plane: &Plane,
    size: (u32, u32),
    colors: (Option<Hdr>, Option<Hdr>),
    offset: u64,
    target: Target,
) -> Made {
    let size = plane_size(plane, size);
    let texture = (target != Target::Surface).then(|| {
        gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vidcut encoding"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: plane.format,
            usage: if target == Target::Held || plane.packed {
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING
            } else {
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC
            },
            view_formats: &[],
        })
    });
    let (row, padded) = if plane.packed {
        (size.0 * 3, size.0 * 3)
    } else {
        let row = size.0 * plane.texel;
        let aligned = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        (row, row.div_ceil(aligned) * aligned)
    };
    let length =
        (u64::from(padded) * u64::from(size.1)).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
    let buffer = (target == Target::Memory).then(|| {
        gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vidcut encoding"),
            size: length,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        })
    });
    let packed = (plane.packed && target == Target::Memory).then(|| {
        gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vidcut encoding"),
            size: length,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    });
    let bytes = params(layout, plane, colors);
    let params = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vidcut encoding"),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    gpu.queue.write_buffer(&params, 0, &bytes);
    Made {
        texture,
        buffer,
        packed,
        params,
        size,
        padded,
        row,
        offset,
    }
}
