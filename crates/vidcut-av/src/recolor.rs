use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::CFRetained;
use objc2_core_video::CVPixelBufferGetIOSurface;
use objc2_foundation::NSString;
use objc2_io_surface::IOSurfaceRef;
use objc2_metal::{
    MTLCommandBuffer, MTLCommandBufferStatus, MTLCommandEncoder, MTLCommandQueue,
    MTLComputeCommandEncoder, MTLComputePipelineState, MTLCreateSystemDefaultDevice, MTLDevice,
    MTLLibrary, MTLPixelFormat, MTLSize, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};

use crate::color::{Matrix, Recoloring};
use crate::error::{Error, Result};
use crate::ffmpeg::{ffi, format, frame::Video};
use crate::frame_processor::{Frames, pixels};
use vidcut_core::model::Hdr;

const WHAT: &str = "color conversion on Metal";

const KERNEL: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct Params {
    float4 in_range;
    float4 in_matrix;
    float4 out_range;
    float4 out_matrix;
    float4 gamut0;
    float4 gamut1;
    float4 gamut2;
    float4 tone;
    float4 scale;
    uint4 flags;
    uint4 shift;
};

float3 decode(float3 c, uint t, float s) {
    c = max(c, float3(0.0));
    if (t == 1u) {
        float3 p = pow(c, float3(1.0 / 78.84375));
        float3 num = max(p - 0.8359375, float3(0.0));
        float3 den = max(18.8515625 - 18.6875 * p, float3(1.17549435e-38));
        return pow(num / den, float3(1.0 / 0.1593017578125)) * s;
    }
    if (t == 2u) {
        float3 high = (exp((c - 0.55991073) / 0.17883277) + 0.28466892) / 12.0;
        float3 l = select(high, c * c / 3.0, c <= 0.5);
        return pow(l, float3(1.2)) * s;
    }
    return pow(c, float3(2.4));
}

float3 encode(float3 c, uint t, float s) {
    if (t == 1u) {
        float3 l = c * s;
        float3 p = pow(max(l, float3(0.0)), float3(0.1593017578125));
        float3 q = pow((0.8359375 + 18.8515625 * p) / (1.0 + 18.6875 * p), float3(78.84375));
        return select(float3(0.0), q, l > 0.0);
    }
    if (t == 2u) {
        float3 l = pow(max(c * s, float3(0.0)), float3(1.0 / 1.2));
        float3 high = 0.17883277 * log(max(12.0 * l - 0.28466892, float3(1e-30))) + 0.55991073;
        return select(high, sqrt(3.0 * l), l <= 1.0 / 12.0);
    }
    return pow(max(c, float3(0.0)), float3(1.0 / 2.4));
}

float3 convert(float3 s, constant Params& p) {
    float y = (s.x - p.in_range.x) * p.in_range.y;
    float cb = (s.y - p.in_range.z) * p.in_range.w;
    float cr = (s.z - p.in_range.z) * p.in_range.w;
    float kr = p.in_matrix.x;
    float kb = p.in_matrix.y;
    float kg = p.in_matrix.z;
    float r = y + (2.0 - 2.0 * kr) * cr;
    float b = y + (2.0 - 2.0 * kb) * cb;
    float g = (y - kr * r - kb * b) / kg;
    float3 c = float3(r, g, b);
    if (p.flags.x != 0u) {
        c = decode(c, p.flags.y, p.scale.x);
        c = float3(dot(p.gamut0.xyz, c), dot(p.gamut1.xyz, c), dot(p.gamut2.xyz, c));
        if (p.flags.w != 0u) {
            float sig = max(max(max(c.r, c.g), c.b), 1e-6);
            float mapped = sig <= p.tone.x ? sig : p.tone.y * (sig + p.tone.z) / (sig + p.tone.w);
            c *= mapped / sig;
        }
        c = encode(c, p.flags.z, p.scale.y);
    }
    float okr = p.out_matrix.x;
    float okb = p.out_matrix.y;
    float okg = p.out_matrix.z;
    float oy = okr * c.r + okg * c.g + okb * c.b;
    float ocb = (c.b - oy) / (2.0 - 2.0 * okb);
    float ocr = (c.r - oy) / (2.0 - 2.0 * okr);
    return float3(
        oy / p.out_range.y + p.out_range.x,
        ocb / p.out_range.w + p.out_range.z,
        ocr / p.out_range.w + p.out_range.z
    );
}

float2 chroma_at(texture2d<float, access::read> chroma, uint2 l, uint2 shift) {
    float2 size = float2(chroma.get_width(), chroma.get_height());
    float2 at = (float2(l) + 0.5) / float2(1u << shift.x, 1u << shift.y) - 0.5;
    at = clamp(at, float2(0.0), size - 1.0);
    uint2 a = uint2(floor(at));
    uint2 b = min(a + 1u, uint2(size) - 1u);
    float2 f = at - float2(a);
    float2 top = mix(chroma.read(a).rg, chroma.read(uint2(b.x, a.y)).rg, float2(f.x));
    float2 bottom = mix(chroma.read(uint2(a.x, b.y)).rg, chroma.read(b).rg, float2(f.x));
    return mix(top, bottom, float2(f.y));
}

kernel void recolor(
    texture2d<float, access::read> luma [[texture(0)]],
    texture2d<float, access::read> chroma [[texture(1)]],
    texture2d<float, access::write> out_luma [[texture(2)]],
    texture2d<float, access::write> out_chroma [[texture(3)]],
    constant Params& p [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]])
{
    if (gid.x >= out_chroma.get_width() || gid.y >= out_chroma.get_height()) {
        return;
    }
    uint bw = 1u << p.shift.z;
    uint bh = 1u << p.shift.w;
    float2 sum = float2(0.0);
    float n = 0.0;
    for (uint j = 0u; j < bh; j++) {
        for (uint i = 0u; i < bw; i++) {
            uint2 l = uint2(gid.x * bw + i, gid.y * bh + j);
            if (l.x >= out_luma.get_width() || l.y >= out_luma.get_height()) {
                continue;
            }
            float3 o = convert(float3(luma.read(l).r, chroma_at(chroma, l, p.shift.xy)), p);
            out_luma.write(float4(clamp(o.x, 0.0, 1.0)), l);
            sum += o.yz;
            n += 1.0;
        }
    }
    out_chroma.write(float4(clamp(sum / max(n, 1.0), float2(0.0), float2(1.0)), 0.0, 0.0), gid);
}
"#;

#[repr(C)]
struct Params {
    in_range: [f32; 4],
    in_matrix: [f32; 4],
    out_range: [f32; 4],
    out_matrix: [f32; 4],
    gamut: [[f32; 4]; 3],
    tone: [f32; 4],
    scale: [f32; 4],
    flags: [u32; 4],
    shift: [u32; 4],
}

pub(crate) struct Recolor {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    pipeline: Retained<ProtocolObject<dyn MTLComputePipelineState>>,
    frames: Frames,
}

// SAFETY: one thread at a time uses a conversion, as the fitting that holds it.
unsafe impl Send for Recolor {}

fn refused(what: &str) -> Error {
    Error::Invalid(format!("{WHAT}: {what}"))
}

struct Layout {
    planes: [MTLPixelFormat; 2],
    shift: (u32, u32),
    wide: bool,
    ten: bool,
}

fn layout(p: format::Pixel) -> Result<Layout> {
    use format::Pixel as P;
    let (planes, shift) = match p {
        P::NV12 => ([MTLPixelFormat::R8Unorm, MTLPixelFormat::RG8Unorm], (1, 1)),
        P::NV16 => ([MTLPixelFormat::R8Unorm, MTLPixelFormat::RG8Unorm], (1, 0)),
        P::NV24 => ([MTLPixelFormat::R8Unorm, MTLPixelFormat::RG8Unorm], (0, 0)),
        P::P010LE => (
            [MTLPixelFormat::R16Unorm, MTLPixelFormat::RG16Unorm],
            (1, 1),
        ),
        P::P210LE => (
            [MTLPixelFormat::R16Unorm, MTLPixelFormat::RG16Unorm],
            (1, 0),
        ),
        P::P416LE => (
            [MTLPixelFormat::R16Unorm, MTLPixelFormat::RG16Unorm],
            (0, 0),
        ),
        other => return Err(refused(&format!("no planes for {other:?}"))),
    };
    Ok(Layout {
        planes,
        shift,
        wide: planes[0] == MTLPixelFormat::R16Unorm,
        ten: matches!(p, P::P010LE | P::P210LE),
    })
}

fn range(l: &Layout, full: bool) -> [f32; 4] {
    let (top, step): (f64, f64) = match (l.wide, l.ten) {
        (false, _) => (255.0, 1.0),
        (true, true) => (65535.0, 64.0),
        (true, false) => (65535.0, 256.0),
    };
    let (y0, y_span, c0, c_span): (f64, f64, f64, f64) = match (full, l.ten) {
        (true, true) => (0.0, 1023.0, 512.0, 1023.0),
        (true, false) => (0.0, 255.0, 128.0, 255.0),
        (false, true) => (64.0, 876.0, 512.0, 896.0),
        (false, false) => (16.0, 219.0, 128.0, 224.0),
    };
    let unit = step / top;
    [
        (y0 * unit) as f32,
        (1.0 / (y_span * unit)) as f32,
        (c0 * unit) as f32,
        (1.0 / (c_span * unit)) as f32,
    ]
}

fn matrix(m: Matrix) -> [f32; 4] {
    let (kr, kb) = match m {
        Matrix::Bt601 => (0.299, 0.114),
        Matrix::Bt709 => (0.2126, 0.0722),
        Matrix::Bt2020 => (0.2627, 0.0593),
    };
    [kr as f32, kb as f32, (1.0 - kr - kb) as f32, 0.0]
}

fn transfer(hdr: Option<Hdr>) -> u32 {
    match hdr {
        Some(Hdr::Pq) => 1,
        Some(Hdr::Hlg) => 2,
        None => 0,
    }
}

fn surface(f: &Video) -> Result<CFRetained<IOSurfaceRef>> {
    CVPixelBufferGetIOSurface(Some(pixels(f, WHAT)?))
        .ok_or_else(|| refused("a pixel buffer without an IOSurface"))
}

impl Recolor {
    pub(crate) fn new() -> Result<Self> {
        let device = MTLCreateSystemDefaultDevice().ok_or_else(|| refused("no Metal device"))?;
        let queue = device
            .newCommandQueue()
            .ok_or_else(|| refused("no command queue"))?;
        let library = device
            .newLibraryWithSource_options_error(&NSString::from_str(KERNEL), None)
            .map_err(|e| crate::frame_processor::failed(WHAT, e))?;
        let function = library
            .newFunctionWithName(&NSString::from_str("recolor"))
            .ok_or_else(|| refused("no kernel"))?;
        let pipeline = device
            .newComputePipelineStateWithFunction_error(&function)
            .map_err(|e| crate::frame_processor::failed(WHAT, e))?;
        Ok(Self {
            device,
            queue,
            pipeline,
            frames: Frames::default(),
        })
    }

    fn texture(
        &self,
        surface: &IOSurfaceRef,
        plane: usize,
        format: MTLPixelFormat,
        usage: MTLTextureUsage,
    ) -> Result<Retained<ProtocolObject<dyn MTLTexture>>> {
        let (w, h) = (
            surface.width_of_plane(plane),
            surface.height_of_plane(plane),
        );
        // SAFETY: `desc` is the plane's size and format; Metal checks it against `surface`.
        unsafe {
            let desc =
                MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                    format, w, h, false,
                );
            desc.setUsage(usage);
            self.device
                .newTextureWithDescriptor_iosurface_plane(&desc, surface, plane)
        }
        .ok_or_else(|| refused("Metal won't make a texture of the IOSurface"))
    }

    pub(crate) fn run(
        &mut self,
        f: &Video,
        colors: &Recoloring,
        to: format::Pixel,
        target: Option<Hdr>,
    ) -> Result<Video> {
        let clock = std::time::Instant::now();
        let (from, into) = (layout(crate::hw::sw_format(f))?, layout(to)?);
        let size = (f.width(), f.height());
        let limited = ffi::AVColorRange::AVCOL_RANGE_MPEG;
        let shape = (size, ffi::AVPixelFormat::from(to));
        let mut out = self.frames.fresh_in(f, shape, limited, WHAT)?;
        let (a, b) = (surface(f)?, surface(&out)?);
        if a.plane_count() != 2 || b.plane_count() != 2 {
            return Err(refused("IOSurfaces of other than two planes"));
        }
        let read = MTLTextureUsage::ShaderRead;
        let write = MTLTextureUsage::ShaderWrite;
        let textures = [
            self.texture(&a, 0, from.planes[0], read)?,
            self.texture(&a, 1, from.planes[1], read)?,
            self.texture(&b, 0, into.planes[0], write)?,
            self.texture(&b, 1, into.planes[1], write)?,
        ];
        let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let linear = colors.linear.as_ref();
        let gamut = linear.map_or(identity, |l| l.gamut);
        let row = |r: [f64; 3]| [r[0] as f32, r[1] as f32, r[2] as f32, 0.0];
        let params = Params {
            in_range: range(&from, colors.full_range),
            in_matrix: matrix(colors.matrix),
            out_range: range(&into, false),
            out_matrix: matrix(colors.out_matrix),
            gamut: gamut.map(row),
            tone: linear
                .and_then(|l| l.tone)
                .map_or([0.0; 4], |t| t.map(|v| v as f32)),
            scale: [
                linear.map_or(1.0, |l| l.from_scale) as f32,
                linear.map_or(1.0, |l| l.to_scale) as f32,
                0.0,
                0.0,
            ],
            flags: [
                u32::from(linear.is_some()),
                linear.map_or(0, |l| transfer(l.from)),
                linear.map_or(0, |l| transfer(l.to)),
                u32::from(linear.is_some_and(|l| l.tone.is_some())),
            ],
            shift: [from.shift.0, from.shift.1, into.shift.0, into.shift.1],
        };
        let buffer = self
            .queue
            .commandBuffer()
            .ok_or_else(|| refused("no command buffer"))?;
        let encoder = buffer
            .computeCommandEncoder()
            .ok_or_else(|| refused("no compute encoder"))?;
        encoder.setComputePipelineState(&self.pipeline);
        let chroma = (textures[3].width(), textures[3].height());
        // SAFETY: live textures of this device, kept until the work ends, and parameters that the call copies.
        unsafe {
            for (i, t) in textures.iter().enumerate() {
                encoder.setTexture_atIndex(Some(t), i);
            }
            encoder.setBytes_length_atIndex(
                NonNull::from(&params).cast(),
                std::mem::size_of::<Params>(),
                0,
            );
        }
        let group = self.pipeline.threadExecutionWidth().max(1);
        let tall = (self.pipeline.maxTotalThreadsPerThreadgroup() / group).max(1);
        encoder.dispatchThreads_threadsPerThreadgroup(
            MTLSize {
                width: chroma.0,
                height: chroma.1,
                depth: 1,
            },
            MTLSize {
                width: group,
                height: tall,
                depth: 1,
            },
        );
        encoder.endEncoding();
        buffer.commit();
        buffer.waitUntilCompleted();
        drop(textures);
        if buffer.status() != MTLCommandBufferStatus::Completed {
            return Err(buffer.error().map_or_else(
                || refused("the conversion did not end"),
                |e| crate::frame_processor::failed(WHAT, e),
            ));
        }
        crate::color::tag_frame(&mut out, target);
        crate::debug!(
            "lanczos",
            "colors of {}x{} into {to:?} in {:.1} ms",
            size.0,
            size.1,
            clock.elapsed().as_secs_f64() * 1e3
        );
        Ok(out)
    }
}
