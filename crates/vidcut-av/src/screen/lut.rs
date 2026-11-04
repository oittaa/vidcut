use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, Mutex};

use vidcut_core::cube::{self, Cube, Table};

use super::{Gpu, MIXING, shader};
use crate::error::{Error, Result};

const PARAMS: usize = 32;

const TEXEL: u32 = 16;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

struct Loaded {
    stamp: String,
    cube: Cube,
    bound: Mutex<Option<(wgpu::Device, wgpu::BindGroup)>>,
}

#[derive(Clone)]
pub struct Lut(Arc<Loaded>);

impl Lut {
    pub(crate) fn stamp(&self) -> &str {
        &self.0.stamp
    }
}

impl std::fmt::Debug for Lut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Lut").field(&self.0.stamp).finish()
    }
}

impl PartialEq for Lut {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Lut {}

impl Hash for Lut {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
    }
}

pub fn load(path: &Path) -> Result<Lut> {
    let stamp = cube::stamp(path);
    let bytes =
        std::fs::read(path).map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?;
    let cube = cube::read(path, &bytes).map_err(Error::Invalid)?;
    Ok(Lut(Arc::new(Loaded {
        stamp,
        cube,
        bound: Mutex::new(None),
    })))
}

fn texels(rows: &[[f32; 3]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rows.len() * TEXEL as usize);
    for [r, g, b] in rows {
        for v in [*r, *g, *b, 1.0] {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

fn table_texture(
    gpu: &Gpu,
    (width, height, depth): (u32, u32, u32),
    dimension: wgpu::TextureDimension,
) -> wgpu::Texture {
    gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vidcut lut"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: depth,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension,
        format: FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn rows_view(gpu: &Gpu, table: &Table) -> (wgpu::TextureView, u32) {
    let width = table.size.min(gpu.device.limits().max_texture_dimension_2d);
    let height = table.size.div_ceil(width);
    let texture = table_texture(gpu, (width, height, 1), wgpu::TextureDimension::D2);
    let mut bytes = texels(&table.rows);
    bytes.resize((width * height * TEXEL) as usize, 0);
    gpu.queue.write_texture(
        texture.as_image_copy(),
        &bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * TEXEL),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    (texture.create_view(&Default::default()), width)
}

fn cube_view(gpu: &Gpu, table: &Table) -> wgpu::TextureView {
    let n = table.size;
    let texture = table_texture(gpu, (n, n, n), wgpu::TextureDimension::D3);
    let layer = n as usize * n as usize;
    for (z, rows) in table.rows.chunks(layer).enumerate() {
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: z as u32,
                },
                ..texture.as_image_copy()
            },
            &texels(rows),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(n * TEXEL),
                rows_per_image: Some(n),
            },
            wgpu::Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: 1,
            },
        );
    }
    texture.create_view(&Default::default())
}

pub(super) struct Pipes {
    layout: wgpu::BindGroupLayout,
    pub(super) converting: wgpu::RenderPipeline,
    pub(super) timeline: wgpu::RenderPipeline,
    no_rows: wgpu::TextureView,
    no_cube: wgpu::TextureView,
}

impl Pipes {
    pub(super) fn new(gpu: &Gpu) -> Self {
        let device = &gpu.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vidcut lut"),
            source: wgpu::ShaderSource::Wgsl(shader().into()),
        });
        let table = |binding, view_dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vidcut lut"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                table(2, wgpu::TextureViewDimension::D2),
                table(3, wgpu::TextureViewDimension::D3),
            ],
        });
        let made = |first: &wgpu::BindGroupLayout, entry: &str| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("vidcut lut"),
                bind_group_layouts: &[Some(first), Some(&layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("vidcut lut"),
                layout: Some(&pipeline_layout),
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
        let converting = made(&gpu.layout, "fs_convert_lut");
        let timeline = made(&gpu.screen_layout, "fs_lut");
        let no_rows = table_texture(gpu, (1, 1, 1), wgpu::TextureDimension::D2)
            .create_view(&Default::default());
        let no_cube = table_texture(gpu, (1, 1, 1), wgpu::TextureDimension::D3)
            .create_view(&Default::default());
        Self {
            layout,
            converting,
            timeline,
            no_rows,
            no_cube,
        }
    }

    pub(super) fn table(&self, gpu: &Gpu, lut: &Lut) -> wgpu::BindGroup {
        let mut bound = lut.0.bound.lock().unwrap_or_else(|e| e.into_inner());
        match &*bound {
            Some((device, bind)) if *device == gpu.device => bind.clone(),
            _ => {
                let bind = self.uploaded(gpu, &lut.0.cube);
                *bound = Some((gpu.device.clone(), bind.clone()));
                bind
            }
        }
    }

    fn uploaded(&self, gpu: &Gpu, table: &Cube) -> wgpu::BindGroup {
        let rows = table.one.as_ref().map(|t| rows_view(gpu, t));
        let three = table.three.as_ref().map(|t| cube_view(gpu, t));
        let range = |t: &Option<Table>| t.as_ref().map_or([0.0, 1.0], |t| t.range);
        let size = |t: &Option<Table>| t.as_ref().map_or(0, |t| t.size);
        let width = rows.as_ref().map_or(1, |(_, w)| *w);
        let mut bytes = Vec::with_capacity(PARAMS);
        for v in range(&table.one).into_iter().chain(range(&table.three)) {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        for v in [size(&table.one), size(&table.three), width, 0] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let params = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vidcut lut"),
            size: PARAMS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue.write_buffer(&params, 0, &bytes);
        gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vidcut lut"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(
                        rows.as_ref().map_or(&self.no_rows, |(v, _)| v),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(
                        three.as_ref().unwrap_or(&self.no_cube),
                    ),
                },
            ],
        })
    }
}

pub(super) struct LookedUp {
    pub(super) size: (u32, u32),
    pub(super) texture: wgpu::Texture,
    pub(super) view: wgpu::TextureView,
}

impl LookedUp {
    pub(super) fn new(gpu: &Gpu, size: (u32, u32)) -> Self {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vidcut output lut"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: MIXING,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self {
            size,
            texture,
            view,
        }
    }
}
