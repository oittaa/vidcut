use wgpu::util::DeviceExt;

use super::Gpu;
use crate::error::{Error, Result};

const LEVELS: u32 = 5;

pub(super) struct Pipes(wgpu::ComputePipeline);

impl Pipes {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vidcut color fix"),
            source: wgpu::ShaderSource::Wgsl(include_str!("fix.wgsl").into()),
        });
        Self(
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("vidcut color fix"),
                layout: None,
                module: &module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            }),
        )
    }
}

pub(crate) fn run(
    gpu: &Gpu,
    (source, made, out): (&wgpu::Texture, &wgpu::Texture, &wgpu::Texture),
) -> Result<()> {
    let pipe = &gpu.fix.get_or_init(|| Pipes::new(&gpu.device)).0;
    let (w, h) = (made.width(), made.height());
    let low = || {
        gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vidcut color fix"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        })
    };
    let (sw, sh) = (source.width(), source.height());
    let view = |t: &wgpu::Texture| t.create_view(&Default::default());
    let (a, b) = (view(&low()), view(&low()));
    let (source, made, out) = (view(source), view(made), view(out));
    let mut steps = vec![(0, 0, &made, &a)];
    for level in 0..LEVELS {
        steps.push((1, 1 << level, &a, &b));
        steps.push((2, 1 << level, &b, &a));
    }
    steps.push((3, 0, &a, &out));
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("vidcut color fix"),
        });
    for (stage, step, from, to) in steps {
        let params: [u32; 8] = [w, h, sw, sh, stage, step, 0, 0];
        let bytes: Vec<u8> = params.iter().flat_map(|v| v.to_le_bytes()).collect();
        let params = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("vidcut color fix"),
                contents: &bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let entries = [
            params.as_entire_binding(),
            wgpu::BindingResource::TextureView(&source),
            wgpu::BindingResource::TextureView(&made),
            wgpu::BindingResource::TextureView(from),
            wgpu::BindingResource::TextureView(to),
        ];
        let entries: Vec<_> = entries
            .into_iter()
            .enumerate()
            .map(|(i, resource)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource,
            })
            .collect();
        let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vidcut color fix"),
            layout: &pipe.get_bind_group_layout(0),
            entries: &entries,
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("vidcut color fix"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipe);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(w.div_ceil(16), h.div_ceil(16), 1);
    }
    let index = gpu.queue.submit([encoder.finish()]);
    gpu.device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(index),
            timeout: None,
        })
        .map(drop)
        .map_err(|e| Error::Invalid(format!("the color fix did not finish: {e}")))
}

#[cfg(target_os = "macos")]
pub(crate) fn on_buffers(
    source: &objc2_core_video::CVPixelBuffer,
    made: &objc2_core_video::CVPixelBuffer,
    out: &objc2_core_video::CVPixelBuffer,
) -> Result<()> {
    let gpu = &super::offscreen().map_err(Error::Invalid)?.gpu;
    let source = imported(gpu, source, false)?;
    let made = imported(gpu, made, false)?;
    let out = imported(gpu, out, true)?;
    run(gpu, (&source, &made, &out))
}

#[cfg(target_os = "macos")]
fn imported(
    gpu: &Gpu,
    buffer: &objc2_core_video::CVPixelBuffer,
    write: bool,
) -> Result<wgpu::Texture> {
    use objc2_metal::{MTLPixelFormat, MTLTextureUsage};
    const WHAT: &str = "the color fix of the scaler of macOS";
    let surface = objc2_core_video::CVPixelBufferGetIOSurface(Some(buffer))
        .ok_or_else(|| Error::Invalid(format!("{WHAT}: a picture buffer without an IOSurface")))?;
    let uses = match write {
        true => (
            MTLTextureUsage::ShaderWrite,
            wgpu::TextureUsages::STORAGE_BINDING,
            wgpu::TextureUses::STORAGE_WRITE_ONLY,
        ),
        false => (
            MTLTextureUsage::ShaderRead,
            wgpu::TextureUsages::TEXTURE_BINDING,
            wgpu::TextureUses::RESOURCE,
        ),
    };
    let half = (
        0,
        MTLPixelFormat::RGBA16Float,
        wgpu::TextureFormat::Rgba16Float,
    );
    gpu.checked(|| crate::metal::imported(&gpu.device, &surface, half, uses, WHAT))
        .map_err(Error::Invalid)?
}
