use crate::compute::Gpu;

pub(crate) const LEVELS: u32 = 5;

pub(crate) enum Pictures<'a> {
    Packed {
        frames: &'a wgpu::Buffer,
        made: &'a wgpu::Buffer,
        out: &'a wgpu::Buffer,
        wm: u32,
        base: u32,
    },
    #[cfg(target_os = "macos")]
    Textures {
        source: &'a wgpu::Texture,
        made: &'a wgpu::Texture,
        out: &'a wgpu::Texture,
    },
}

fn shader(form: &str) -> String {
    format!("{form}\n{}", include_str!("fix.wgsl"))
}

pub(crate) fn run(
    gpu: &Gpu,
    pass: &mut wgpu::ComputePass,
    pictures: &Pictures,
    (a, b): (&wgpu::Buffer, &wgpu::Buffer),
    (w, h): (u32, u32),
) {
    let pixels = w * h;
    let mut steps = vec![(0, 0, (b, a))];
    for level in 0..LEVELS {
        steps.push((1, 1 << level, (a, b)));
        steps.push((2, 1 << level, (b, a)));
    }
    steps.push((3, 0, (a, b)));
    match pictures {
        Pictures::Packed {
            frames,
            made,
            out,
            wm,
            base,
        } => {
            let pipe = gpu.pipe("fix", || shader(include_str!("fix_packed.wgsl")));
            let words = (pixels * 3).div_ceil(2);
            for (stage, step, (low, lower)) in steps {
                let count = if stage == 3 { words } else { pixels };
                let p = gpu.uniform(&[w, h, *wm, *base, 0, stage, step, count]);
                let buffers = [*frames, *made, low, lower, *out];
                gpu.dispatch(pass, &pipe, p, &buffers, count.div_ceil(256));
            }
        }
        #[cfg(target_os = "macos")]
        Pictures::Textures { source, made, out } => {
            let pipe = gpu.pipe("fix textures", || shader(include_str!("fix_textures.wgsl")));
            let view = |t: &wgpu::Texture| t.create_view(&Default::default());
            let (source_view, made_view, out_view) = (view(source), view(made), view(out));
            let (sw, sh) = (source.width(), source.height());
            for (stage, step, (low, lower)) in steps {
                let p = gpu.uniform(&[w, h, sw, sh, 0, stage, step, pixels]);
                let resources = vec![
                    wgpu::BindingResource::TextureView(&source_view),
                    wgpu::BindingResource::TextureView(&made_view),
                    low.as_entire_binding(),
                    lower.as_entire_binding(),
                    wgpu::BindingResource::TextureView(&out_view),
                ];
                gpu.dispatch_bound(pass, &pipe, p, resources, pixels.div_ceil(256));
            }
        }
    }
}

#[cfg(target_os = "macos")]
const WHAT: &str = "the color fix of the scaler of macOS";

#[cfg(target_os = "macos")]
pub(crate) fn on_buffers(
    gpu: &Gpu,
    (source, made, out): (
        &objc2_core_video::CVPixelBuffer,
        &objc2_core_video::CVPixelBuffer,
        &objc2_core_video::CVPixelBuffer,
    ),
) -> crate::error::Result<()> {
    let [source, made, out] = crate::compute::checked(&gpu.device, WHAT, || {
        Ok([
            imported(gpu, source, false)?,
            imported(gpu, made, false)?,
            imported(gpu, out, true)?,
        ])
    })?;
    let (w, h) = (made.width(), made.height());
    let bytes = u64::from(w * h * 3 * 4);
    let (a, b) = gpu.scratch(bytes, bytes);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        let pictures = Pictures::Textures {
            source: &source,
            made: &made,
            out: &out,
        };
        run(gpu, &mut pass, &pictures, (&a, &b), (w, h));
    }
    gpu.flush();
    let done = gpu.queue.submit([encoder.finish()]);
    gpu.wait_for(done)
}

#[cfg(target_os = "macos")]
fn imported(
    gpu: &Gpu,
    buffer: &objc2_core_video::CVPixelBuffer,
    write: bool,
) -> crate::error::Result<wgpu::Texture> {
    use objc2_metal::{MTLPixelFormat, MTLTextureUsage};
    let surface = objc2_core_video::CVPixelBufferGetIOSurface(Some(buffer)).ok_or_else(|| {
        crate::error::Error::Invalid(format!("{WHAT}: a picture buffer without an IOSurface"))
    })?;
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
    crate::metal::imported(&gpu.device, &surface, half, uses, WHAT)
}
