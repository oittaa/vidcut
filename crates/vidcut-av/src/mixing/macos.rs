use std::ptr::NonNull;

use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLComputeCommandEncoder,
    MTLComputePipelineState, MTLDevice, MTLOrigin, MTLPixelFormat, MTLRegion, MTLSize, MTLTexture,
    MTLTextureDescriptor, MTLTextureUsage,
};

use crate::error::{Error, Result};
use crate::export::mix::Samples;
use crate::export::place::GpuPlane;
use crate::export::video::frame_ref;
use crate::ffmpeg::{ffi, format, frame::Video};
use crate::frame_processor::Frames;
use crate::metal::{Device, Pipeline, Queue, Texture, surface, texture};

const WHAT: &str = "mixing pictures on Metal";

const KERNEL: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct Sums {
    uint4 sums;
    uint4 flags;
};

struct Cover {
    uint4 sums;
    uint4 area;
};

uint2 code(texture2d<uint, access::read> t, uint2 g, uint black, uint shift, uint value) {
    return black != 0u ? uint2(value) : t.read(g).xy >> uint2(shift);
}

kernel void dissolve(
    texture2d<uint, access::read> a [[texture(0)]],
    texture2d<uint, access::read> b [[texture(1)]],
    texture2d<uint, access::read> c [[texture(2)]],
    texture2d<uint, access::write> out [[texture(3)]],
    constant Sums& p [[buffer(0)]],
    uint2 g [[thread_position_in_grid]])
{
    if (g.x >= out.get_width() || g.y >= out.get_height()) {
        return;
    }
    uint2 x = code(a, g, p.flags.y, p.sums.y, p.sums.w);
    uint2 y = code(b, g, p.flags.z, p.sums.y, p.sums.w);
    uint2 v;
    if (p.flags.x == 0u) {
        v = (x * uint2(65536u - p.sums.x) + y * uint2(p.sums.x) + uint2(32768u)) >> uint2(16u);
    } else {
        int2 z = int2(code(c, g, p.flags.w, p.sums.y, p.sums.w));
        v = uint2(clamp(int2(x) + int2(y) - z, int2(0), int2(int(p.sums.z))));
    }
    out.write(uint4(v << uint2(p.sums.y), 0u, 0u), g);
}

kernel void cover(
    texture2d<uint, access::read> under [[texture(0)]],
    texture2d<uint, access::read> color [[texture(1)]],
    texture2d<uint, access::read> alpha [[texture(2)]],
    texture2d<uint, access::write> out [[texture(3)]],
    constant Cover& p [[buffer(0)]],
    uint2 g [[thread_position_in_grid]])
{
    if (g.x >= out.get_width() || g.y >= out.get_height()) {
        return;
    }
    uint2 v = code(under, g, p.sums.w, p.sums.x, p.sums.z);
    if (all(g >= p.area.xy) && all(g < p.area.zw)) {
        uint2 at = g - p.area.xy;
        float2 c = float2(color.read(at).xy >> uint2(p.sums.x));
        float2 a = float2(alpha.read(at).xy >> uint2(p.sums.x)) / float(p.sums.y);
        float2 u = float2(v);
        v = uint2(rint(fma(c - u, a, u)));
    }
    out.write(uint4(v << uint2(p.sums.x), 0u, 0u), g);
}
"#;

const PLACE: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct Level {
    float back[8];
    float edge[4];
    float inner[4];
    float reach[2];
    float opacity;
};

struct Plane {
    uint width, height;
    uint comps, bytes, shift, top;
    uint x0, y0, x1, y1;
    uint cols, rows;
    uint tx, ty, count;
    float across, down, sa, sd;
    float fitted[4];
    float lo[2], hi[2];
    float level;
    Level levels[8];
};

struct Place {
    Plane p;
    uint black;
    uint value;
};

float2 code(texture2d<uint, access::read> t, uint2 at, constant Plane& p) {
    return float2((t.read(at).xy >> uint2(p.shift)) & uint2(p.top));
}

float soft(constant Level& l, float u, float v) {
    float ru = l.reach[0], rv = l.reach[1];
    if (ru <= 0.0f || rv <= 0.0f) {
        return 1.0f;
    }
    float a = max(max(l.inner[0] - u, u - l.inner[2]), 0.0f) / ru;
    float b = max(max(l.inner[1] - v, v - l.inner[3]), 0.0f) / rv;
    float h = max(0.5f - fabs(a - b), 0.0f) / 0.5f;
    float m = max(a, b) + h * h * 0.5f / 4.0f;
    float t = clamp((1.0f - m) / 0.875f, 0.0f, 1.0f);
    return t * t * (3.0f - 2.0f * t);
}

bool covered(constant Plane& p, float x, float y, thread float& cover) {
    cover = 1.0f;
    for (uint i = 0; i < p.count; i++) {
        constant Level& l = p.levels[i];
        float w = l.back[6] * x + l.back[7] * y + 1.0f;
        if (!isfinite(w) || fabs(w) < 1e-12f) {
            return false;
        }
        float u = (l.back[0] * x + l.back[1] * y + l.back[2]) / w;
        float v = (l.back[3] * x + l.back[4] * y + l.back[5]) / w;
        if (!isfinite(u) || !isfinite(v)) {
            return false;
        }
        float ua = l.back[0] - u * l.back[6], ub = l.back[1] - u * l.back[7];
        float va = l.back[3] - v * l.back[6], vb = l.back[4] - v * l.back[7];
        float du = sqrt(ua * ua + ub * ub) / fabs(w);
        float dv = sqrt(va * va + vb * vb) / fabs(w);
        cover *= clamp((u - l.edge[0]) / du + 0.5f, 0.0f, 1.0f);
        cover *= clamp((l.edge[2] - u) / du + 0.5f, 0.0f, 1.0f);
        cover *= clamp((v - l.edge[1]) / dv + 0.5f, 0.0f, 1.0f);
        cover *= clamp((l.edge[3] - v) / dv + 0.5f, 0.0f, 1.0f);
        cover *= soft(l, u, v) * l.opacity;
        if (!(cover > 0.0f)) {
            return false;
        }
    }
    return true;
}

float2 sampled(texture2d<uint, access::read> src, constant Plane& p, uint2 g) {
    constant Level& l = p.levels[p.count - 1];
    float2 sum = float2(0.0f);
    for (uint j = 0; j < p.ty; j++) {
        for (uint i = 0; i < p.tx; i++) {
            float px = float(g.x) * p.across + (float(i) + 0.5f) / float(p.tx) * p.across;
            float py = float(g.y) * p.down + (float(j) + 0.5f) / float(p.ty) * p.down;
            float w = l.back[6] * px + l.back[7] * py + 1.0f;
            if (!isfinite(w) || fabs(w) < 1e-12f) {
                continue;
            }
            float u = (l.back[0] * px + l.back[1] * py + l.back[2]) / w;
            float v = (l.back[3] * px + l.back[4] * py + l.back[5]) / w;
            float sx = clamp((p.fitted[0] + u * p.fitted[2]) / p.sa - 0.5f, p.lo[0],
                             max(p.hi[0], p.lo[0]));
            float sy = clamp((p.fitted[1] + v * p.fitted[3]) / p.sd - 0.5f, p.lo[1],
                             max(p.hi[1], p.lo[1]));
            float kx = floor(sx), ky = floor(sy);
            float ax = sx - kx, ay = sy - ky;
            uint c0 = uint(kx), r0 = uint(ky);
            uint c1 = min(c0 + 1u, p.cols - 1u);
            uint r1 = min(r0 + 1u, p.rows - 1u);
            float2 top = code(src, uint2(c0, r0), p) * (1.0f - ax) + code(src, uint2(c1, r0), p) * ax;
            float2 bottom = code(src, uint2(c0, r1), p) * (1.0f - ax) + code(src, uint2(c1, r1), p) * ax;
            sum += top * (1.0f - ay) + bottom * ay;
        }
    }
    return sum / float(p.tx * p.ty);
}

kernel void place(
    texture2d<uint, access::read> under [[texture(0)]],
    texture2d<uint, access::read> src [[texture(1)]],
    texture2d<uint, access::write> out [[texture(2)]],
    constant Place& q [[buffer(0)]],
    uint2 g [[thread_position_in_grid]])
{
    constant Plane& p = q.p;
    if (g.x >= p.width || g.y >= p.height) {
        return;
    }
    float2 v = q.black != 0u ? float2(float(q.value)) : code(under, g, p);
    float cover = 0.0f;
    bool inside = g.x >= p.x0 && g.x < p.x1 && g.y >= p.y0 && g.y < p.y1;
    if (inside && covered(p, (float(g.x) + 0.5f) * p.across, (float(g.y) + 0.5f) * p.down, cover)) {
        float2 shown = sampled(src, p, g);
        v = v + (shown - v) * (cover * p.level);
    }
    uint2 c = uint2(clamp(round(v), float2(0.0f), float2(float(p.top)))) << uint2(p.shift);
    out.write(uint4(c, 0u, 0u), g);
}
"#;

#[repr(C)]
struct Sums {
    sums: [u32; 4],
    flags: [u32; 4],
}

#[repr(C)]
struct Place {
    plane: GpuPlane,
    black: u32,
    value: u32,
}

#[repr(C)]
struct Cover {
    sums: [u32; 4],
    area: [u32; 4],
}

pub(crate) struct Mixer {
    device: Device,
    queue: Queue,
    dissolve: Pipeline,
    cover: Pipeline,
    place: Option<Pipeline>,
    frames: Frames,
}

// SAFETY: one thread at a time uses a mixer, as the export that holds it.
unsafe impl Send for Mixer {}

pub(crate) struct Overlay {
    layout: format::Pixel,
    planes: Vec<[Texture; 2]>,
    at: (u32, u32),
}

// SAFETY: one thread at a time uses an overlay, as the export that holds it.
unsafe impl Send for Overlay {}

fn refused(what: &str) -> Error {
    Error::Invalid(format!("{WHAT}: {what}"))
}

struct Layout {
    formats: [MTLPixelFormat; 2],
    samples: Samples,
    shift: (u32, u32),
}

impl Layout {
    fn of(p: format::Pixel) -> Result<Self> {
        let shift = match p {
            format::Pixel::NV12 | format::Pixel::P010LE => (1, 1),
            format::Pixel::NV16 | format::Pixel::P210LE => (1, 0),
            format::Pixel::NV24 | format::Pixel::P416LE => (0, 0),
            other => return Err(refused(&format!("no planes for {other:?}"))),
        };
        let samples = Samples::of(p)?;
        let formats = match samples.bytes {
            1 => [MTLPixelFormat::R8Uint, MTLPixelFormat::RG8Uint],
            _ => [MTLPixelFormat::R16Uint, MTLPixelFormat::RG16Uint],
        };
        Ok(Self {
            formats,
            samples,
            shift,
        })
    }

    fn black(&self) -> [u32; 2] {
        let unit = (self.samples.top + 1) / 256;
        [16 * unit, 128 * unit]
    }
}

fn alike<'a>(like: &Video, frames: impl IntoIterator<Item = &'a Video>) -> Result<format::Pixel> {
    let layout = crate::hw::sw_format(like);
    let size = (like.width(), like.height());
    if frames
        .into_iter()
        .any(|f| crate::hw::sw_format(f) != layout || (f.width(), f.height()) != size)
    {
        return Err(refused("pictures of other sizes or layouts"));
    }
    Ok(layout)
}

fn dispatch(
    encoder: &ProtocolObject<dyn MTLComputeCommandEncoder>,
    pipeline: &Pipeline,
    into: &Texture,
) {
    let group = pipeline.threadExecutionWidth().max(1);
    let tall = (pipeline.maxTotalThreadsPerThreadgroup() / group).max(1);
    encoder.dispatchThreads_threadsPerThreadgroup(
        MTLSize {
            width: into.width(),
            height: into.height(),
            depth: 1,
        },
        MTLSize {
            width: group,
            height: tall,
            depth: 1,
        },
    );
}

fn upload(device: &Device, f: &Video, plane: usize, format: MTLPixelFormat) -> Result<Texture> {
    let (w, h) = (
        f.plane_width(plane) as usize,
        f.plane_height(plane) as usize,
    );
    // SAFETY: a new texture of the plane's size and format.
    let t = unsafe {
        let desc = MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            format, w, h, false,
        );
        desc.setUsage(MTLTextureUsage::ShaderRead);
        device.newTextureWithDescriptor(&desc)
    }
    .ok_or_else(|| refused("no texture"))?;
    let bytes = NonNull::new(f.data(plane).as_ptr().cast_mut())
        .ok_or_else(|| refused("a plane without data"))?;
    let region = MTLRegion {
        origin: MTLOrigin { x: 0, y: 0, z: 0 },
        size: MTLSize {
            width: w,
            height: h,
            depth: 1,
        },
    };
    // SAFETY: the plane holds `h` rows of `stride` bytes, each with `w` samples of `format`.
    unsafe {
        t.replaceRegion_mipmapLevel_withBytes_bytesPerRow(region, 0, bytes.cast(), f.stride(plane))
    };
    Ok(t)
}

impl Mixer {
    pub(crate) fn new() -> Result<Self> {
        let (device, queue) = crate::metal::device(WHAT)?;
        let dissolve = crate::metal::kernel(&device, KERNEL, "dissolve", WHAT)?;
        let cover = crate::metal::kernel(&device, KERNEL, "cover", WHAT)?;
        Ok(Self {
            device,
            queue,
            dissolve,
            cover,
            place: None,
            frames: Frames::default(),
        })
    }

    pub(crate) fn mix(
        &mut self,
        under: Option<&Video>,
        over: Option<&Video>,
        weight: u32,
    ) -> Result<Option<Video>> {
        self.run([under, over, None], 0, weight)
    }

    pub(crate) fn sum(
        &mut self,
        a: Option<&Video>,
        b: Option<&Video>,
        c: Option<&Video>,
    ) -> Result<Option<Video>> {
        self.run([a, b, c], 1, 0)
    }

    fn run(
        &mut self,
        inputs: [Option<&Video>; 3],
        mode: u32,
        weight: u32,
    ) -> Result<Option<Video>> {
        let Some(like) = inputs.into_iter().flatten().next() else {
            return Ok(None);
        };
        let clock = std::time::Instant::now();
        let size = (like.width(), like.height());
        let layout = alike(like, inputs.into_iter().flatten())?;
        let l = Layout::of(layout)?;
        let black = l.black();
        let out = self
            .frames
            .fresh(like, (size, ffi::AVPixelFormat::from(layout)), WHAT)?;
        let from = inputs
            .into_iter()
            .map(|f| f.map(|f| surface(f, WHAT)).transpose())
            .collect::<Result<Vec<_>>>()?;
        let to = surface(&out, WHAT)?;
        let buffer = self
            .queue
            .commandBuffer()
            .ok_or_else(|| refused("no command buffer"))?;
        let encoder = buffer
            .computeCommandEncoder()
            .ok_or_else(|| refused("no compute encoder"))?;
        encoder.setComputePipelineState(&self.dissolve);
        let mut held = Vec::with_capacity(8);
        for (plane, format) in l.formats.into_iter().enumerate() {
            let read = MTLTextureUsage::ShaderRead;
            let textures = from
                .iter()
                .map(|s| {
                    s.as_ref()
                        .map(|s| texture(&self.device, s, (plane, format), read, WHAT))
                        .transpose()
                })
                .collect::<Result<Vec<_>>>()?;
            let stand_in = textures
                .iter()
                .flatten()
                .next()
                .expect("one picture that isn't black")
                .clone();
            let write = MTLTextureUsage::ShaderWrite;
            let into = texture(&self.device, &to, (plane, format), write, WHAT)?;
            let params = Sums {
                sums: [weight, l.samples.shift, l.samples.top, black[plane]],
                flags: [
                    mode,
                    u32::from(inputs[0].is_none()),
                    u32::from(inputs[1].is_none()),
                    u32::from(inputs[2].is_none()),
                ],
            };
            // SAFETY: live textures of this device, kept until the work ends, and parameters that the call copies.
            unsafe {
                for (i, t) in textures.iter().enumerate() {
                    encoder.setTexture_atIndex(Some(t.as_ref().unwrap_or(&stand_in)), i);
                }
                encoder.setTexture_atIndex(Some(&into), 3);
                encoder.setBytes_length_atIndex(
                    NonNull::from(&params).cast(),
                    std::mem::size_of::<Sums>(),
                    0,
                );
            }
            dispatch(&encoder, &self.dissolve, &into);
            held.extend(textures.into_iter().flatten());
            held.extend([stand_in, into]);
        }
        encoder.endEncoding();
        buffer.commit();
        buffer.waitUntilCompleted();
        drop(held);
        crate::metal::ended(&buffer, WHAT)?;
        crate::debug!(
            "mixing",
            "a mix of {}x{} {layout:?} in {:.1} ms",
            size.0,
            size.1,
            clock.elapsed().as_secs_f64() * 1e3
        );
        Ok(Some(out))
    }

    pub(crate) fn burn(
        &mut self,
        under: Option<&Video>,
        like: &Video,
        over: &Overlay,
    ) -> Result<Option<Video>> {
        if over.planes.is_empty() {
            return under.map(frame_ref).transpose();
        }
        let clock = std::time::Instant::now();
        let size = (like.width(), like.height());
        let layout = alike(like, under)?;
        if over.layout != layout {
            return Err(refused("an overlay of another layout"));
        }
        let l = Layout::of(layout)?;
        let black = l.black();
        let mut out = self
            .frames
            .fresh(like, (size, ffi::AVPixelFormat::from(layout)), WHAT)?;
        let from = under.map(|f| surface(f, WHAT)).transpose()?;
        let to = surface(&out, WHAT)?;
        let buffer = self
            .queue
            .commandBuffer()
            .ok_or_else(|| refused("no command buffer"))?;
        let encoder = buffer
            .computeCommandEncoder()
            .ok_or_else(|| refused("no compute encoder"))?;
        encoder.setComputePipelineState(&self.cover);
        let mut held = Vec::with_capacity(4);
        for (plane, format) in l.formats.into_iter().enumerate() {
            let [color, alpha] = &over.planes[plane];
            let read = MTLTextureUsage::ShaderRead;
            let below = from
                .as_ref()
                .map(|s| texture(&self.device, s, (plane, format), read, WHAT))
                .transpose()?;
            let write = MTLTextureUsage::ShaderWrite;
            let into = texture(&self.device, &to, (plane, format), write, WHAT)?;
            let (sx, sy) = if plane == 0 { (0, 0) } else { l.shift };
            let (x, y) = (over.at.0 >> sx, over.at.1 >> sy);
            let params = Cover {
                sums: [
                    l.samples.shift,
                    l.samples.top,
                    black[plane],
                    u32::from(under.is_none()),
                ],
                area: [x, y, x + color.width() as u32, y + color.height() as u32],
            };
            // SAFETY: live textures of this device, kept until the work ends, and parameters that the call copies.
            unsafe {
                encoder.setTexture_atIndex(Some(below.as_ref().unwrap_or(color)), 0);
                encoder.setTexture_atIndex(Some(color), 1);
                encoder.setTexture_atIndex(Some(alpha), 2);
                encoder.setTexture_atIndex(Some(&into), 3);
                encoder.setBytes_length_atIndex(
                    NonNull::from(&params).cast(),
                    std::mem::size_of::<Cover>(),
                    0,
                );
            }
            dispatch(&encoder, &self.cover, &into);
            held.extend(below);
            held.push(into);
        }
        encoder.endEncoding();
        buffer.commit();
        buffer.waitUntilCompleted();
        drop(held);
        crate::metal::ended(&buffer, WHAT)?;
        // SAFETY: both are valid frames.
        unsafe { ffi::av_frame_copy_props(out.as_mut_ptr(), like.as_ptr()) };
        crate::debug!(
            "mixing",
            "a picture over {}x{} {layout:?} in {:.1} ms",
            size.0,
            size.1,
            clock.elapsed().as_secs_f64() * 1e3
        );
        Ok(Some(out))
    }

    pub(crate) fn place(
        &mut self,
        under: Option<&Video>,
        like: &Video,
        layer: &crate::export::place::Layer,
        size: (u32, u32),
    ) -> Result<Option<Video>> {
        let clock = std::time::Instant::now();
        let out_size = (like.width(), like.height());
        let layout = alike(like, under.into_iter().chain([layer.picture]))?;
        let area = layer.area(size).unwrap_or_default();
        let Some(sums) = layer.gpu_planes(out_size, size, area)? else {
            return Ok(None);
        };
        if self.place.is_none() {
            self.place = Some(crate::metal::kernel(&self.device, PLACE, "place", WHAT)?);
        }
        let pipeline = self.place.as_ref().expect("made above");
        let l = Layout::of(layout)?;
        let black = l.black();
        let mut out =
            self.frames
                .fresh(like, (out_size, ffi::AVPixelFormat::from(layout)), WHAT)?;
        let from = under.map(|f| surface(f, WHAT)).transpose()?;
        let src = surface(layer.picture, WHAT)?;
        let to = surface(&out, WHAT)?;
        let buffer = self
            .queue
            .commandBuffer()
            .ok_or_else(|| refused("no command buffer"))?;
        let encoder = buffer
            .computeCommandEncoder()
            .ok_or_else(|| refused("no compute encoder"))?;
        encoder.setComputePipelineState(pipeline);
        let mut held = Vec::with_capacity(6);
        for (plane, format) in l.formats.into_iter().enumerate() {
            let read = MTLTextureUsage::ShaderRead;
            let below = from
                .as_ref()
                .map(|s| texture(&self.device, s, (plane, format), read, WHAT))
                .transpose()?;
            let shown = texture(&self.device, &src, (plane, format), read, WHAT)?;
            let write = MTLTextureUsage::ShaderWrite;
            let into = texture(&self.device, &to, (plane, format), write, WHAT)?;
            let params = Place {
                plane: sums[plane],
                black: u32::from(under.is_none()),
                value: black[plane],
            };
            // SAFETY: live textures of this device, kept until the work ends, and parameters that the call copies.
            unsafe {
                encoder.setTexture_atIndex(Some(below.as_ref().unwrap_or(&shown)), 0);
                encoder.setTexture_atIndex(Some(&shown), 1);
                encoder.setTexture_atIndex(Some(&into), 2);
                encoder.setBytes_length_atIndex(
                    NonNull::from(&params).cast(),
                    std::mem::size_of::<Place>(),
                    0,
                );
            }
            dispatch(&encoder, pipeline, &into);
            held.extend(below);
            held.extend([shown, into]);
        }
        encoder.endEncoding();
        buffer.commit();
        buffer.waitUntilCompleted();
        drop(held);
        crate::metal::ended(&buffer, WHAT)?;
        // SAFETY: both are valid frames.
        unsafe { ffi::av_frame_copy_props(out.as_mut_ptr(), like.as_ptr()) };
        crate::debug!(
            "mixing",
            "a placed picture over {}x{} {layout:?} in {:.1} ms",
            out_size.0,
            out_size.1,
            clock.elapsed().as_secs_f64() * 1e3
        );
        Ok(Some(out))
    }

    fn up(
        &self,
        layout: format::Pixel,
        (color, alpha): (&Video, &Video),
        at: (u32, u32),
    ) -> Result<Overlay> {
        let l = Layout::of(layout)?;
        let mut planes = Vec::with_capacity(2);
        for (plane, format) in l.formats.into_iter().enumerate() {
            planes.push([
                upload(&self.device, color, plane, format)?,
                upload(&self.device, alpha, plane, format)?,
            ]);
        }
        crate::copies::up(color);
        crate::copies::up(alpha);
        Ok(Overlay { layout, planes, at })
    }
}

impl Overlay {
    fn empty(layout: format::Pixel) -> Self {
        Self {
            layout,
            planes: Vec::new(),
            at: (0, 0),
        }
    }

    pub(crate) fn new(
        mixer: &Mixer,
        like: &Video,
        sprites: &[crate::burn::Sprite],
        shift: crate::burn::Placing,
    ) -> Result<Self> {
        let layout = crate::hw::sw_format(like);
        let size = (like.width(), like.height());
        let Some(area) = crate::burn::bounds(sprites, shift, size) else {
            return Ok(Self::empty(layout));
        };
        let (color, alpha) = crate::burn::overlay(layout, like, sprites, shift, area)?;
        mixer.up(layout, (&color, &alpha), (area.0 as u32, area.1 as u32))
    }

    pub(crate) fn of(
        mixer: &Mixer,
        like: &Video,
        layer: &crate::export::place::Layer,
        size: (u32, u32),
    ) -> Result<Self> {
        let layout = crate::hw::sw_format(like);
        let Some(area) = layer.area(size) else {
            return Ok(Self::empty(layout));
        };
        let (color, alpha) = crate::export::place::overlay(layout, layer, size, area)?;
        mixer.up(layout, (&color, &alpha), (area[0], area[1]))
    }
}
