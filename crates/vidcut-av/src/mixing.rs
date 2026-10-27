use std::ptr::NonNull;

use objc2_metal::{
    MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLComputeCommandEncoder,
    MTLComputePipelineState, MTLPixelFormat, MTLSize, MTLTexture, MTLTextureUsage,
};

use crate::error::{Error, Result};
use crate::export::mix::Samples;
use crate::ffmpeg::{ffi, format, frame::Video};
use crate::frame_processor::Frames;
use crate::metal::{Device, Pipeline, Queue, surface, texture};

const WHAT: &str = "mixing pictures on Metal";

const KERNEL: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct Params {
    uint4 sums;
    uint4 flags;
};

uint2 code(texture2d<uint, access::read> t, uint2 g, uint black, constant Params& p) {
    return black != 0u ? uint2(p.sums.w) : t.read(g).xy >> uint2(p.sums.y);
}

kernel void dissolve(
    texture2d<uint, access::read> a [[texture(0)]],
    texture2d<uint, access::read> b [[texture(1)]],
    texture2d<uint, access::read> c [[texture(2)]],
    texture2d<uint, access::write> out [[texture(3)]],
    constant Params& p [[buffer(0)]],
    uint2 g [[thread_position_in_grid]])
{
    if (g.x >= out.get_width() || g.y >= out.get_height()) {
        return;
    }
    uint2 x = code(a, g, p.flags.y, p);
    uint2 y = code(b, g, p.flags.z, p);
    uint2 v;
    if (p.flags.x == 0u) {
        v = (x * uint2(65536u - p.sums.x) + y * uint2(p.sums.x) + uint2(32768u)) >> uint2(16u);
    } else {
        int2 z = int2(code(c, g, p.flags.w, p));
        v = uint2(clamp(int2(x) + int2(y) - z, int2(0), int2(int(p.sums.z))));
    }
    out.write(uint4(v << uint2(p.sums.y), 0u, 0u), g);
}
"#;

#[repr(C)]
struct Params {
    sums: [u32; 4],
    flags: [u32; 4],
}

pub(crate) struct Mixer {
    device: Device,
    queue: Queue,
    pipeline: Pipeline,
    frames: Frames,
}

// SAFETY: one thread at a time uses a mixer, as the export that holds it.
unsafe impl Send for Mixer {}

fn refused(what: &str) -> Error {
    Error::Invalid(format!("{WHAT}: {what}"))
}

impl Mixer {
    pub(crate) fn new() -> Result<Self> {
        let (device, queue) = crate::metal::device(WHAT)?;
        let pipeline = crate::metal::kernel(&device, KERNEL, "dissolve", WHAT)?;
        Ok(Self {
            device,
            queue,
            pipeline,
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
        let layout = crate::hw::sw_format(like);
        let size = (like.width(), like.height());
        if inputs
            .into_iter()
            .flatten()
            .any(|f| crate::hw::sw_format(f) != layout || (f.width(), f.height()) != size)
        {
            return Err(refused("pictures of other sizes or layouts"));
        }
        if !matches!(
            layout,
            format::Pixel::NV12
                | format::Pixel::NV16
                | format::Pixel::P010LE
                | format::Pixel::P210LE
        ) {
            return Err(refused(&format!("no planes for {layout:?}")));
        }
        let samples = Samples::of(layout)?;
        let formats = match samples.bytes {
            1 => [MTLPixelFormat::R8Uint, MTLPixelFormat::RG8Uint],
            _ => [MTLPixelFormat::R16Uint, MTLPixelFormat::RG16Uint],
        };
        let unit = (samples.top + 1) / 256;
        let black = [16 * unit, 128 * unit];
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
        encoder.setComputePipelineState(&self.pipeline);
        let group = self.pipeline.threadExecutionWidth().max(1);
        let tall = (self.pipeline.maxTotalThreadsPerThreadgroup() / group).max(1);
        let mut held = Vec::with_capacity(8);
        for (plane, format) in formats.into_iter().enumerate() {
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
            let params = Params {
                sums: [weight, samples.shift, samples.top, black[plane]],
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
                    std::mem::size_of::<Params>(),
                    0,
                );
            }
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
}
