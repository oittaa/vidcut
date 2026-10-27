use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2_metal::{
    MTLCommandBuffer, MTLCommandQueue, MTLOrigin, MTLPixelFormat, MTLRegion, MTLSize, MTLTexture,
    MTLTextureUsage,
};
use objc2_metal_performance_shaders::{MPSImageLanczosScale, MPSScaleTransform};

use crate::error::{Error, Result};
use crate::ffmpeg::{ffi, format, frame::Video};
use crate::frame_processor::Frames;
use crate::metal::{Device, Queue, surface, texture};

const WHAT: &str = "lanczos scaling on Metal";

pub(crate) struct Lanczos {
    device: Device,
    queue: Queue,
    kernel: Retained<MPSImageLanczosScale>,
    frames: Frames,
}

// SAFETY: one thread at a time uses a scaler, as the fitting that holds it.
unsafe impl Send for Lanczos {}

fn refused(what: &str) -> Error {
    Error::Invalid(format!("{WHAT}: {what}"))
}

fn planes(layout: format::Pixel) -> Result<[MTLPixelFormat; 2]> {
    match layout {
        format::Pixel::NV12 | format::Pixel::NV16 | format::Pixel::NV24 => {
            Ok([MTLPixelFormat::R8Unorm, MTLPixelFormat::RG8Unorm])
        }
        format::Pixel::P010LE | format::Pixel::P210LE | format::Pixel::P416LE => {
            Ok([MTLPixelFormat::R16Unorm, MTLPixelFormat::RG16Unorm])
        }
        other => Err(refused(&format!("no planes for {other:?}"))),
    }
}

fn chroma_shift(layout: format::Pixel) -> (u32, u32) {
    match layout {
        format::Pixel::NV12 | format::Pixel::P010LE => (1, 1),
        format::Pixel::NV16 | format::Pixel::P210LE => (1, 0),
        _ => (0, 0),
    }
}

pub(crate) fn sized(
    lanczos: &mut Option<Lanczos>,
    f: &Video,
    size: (u32, u32),
) -> Result<Option<Video>> {
    if (f.width(), f.height()) == size {
        return Ok(None);
    }
    if lanczos.is_none() {
        *lanczos = Some(Lanczos::new()?);
    }
    lanczos
        .as_mut()
        .expect("made above")
        .scale(f, size)
        .map(Some)
}

impl Lanczos {
    pub(crate) fn new() -> Result<Self> {
        let (device, queue) = crate::metal::device(WHAT)?;
        // SAFETY: a new kernel on a live device.
        let kernel =
            unsafe { MPSImageLanczosScale::initWithDevice(MPSImageLanczosScale::alloc(), &device) };
        Ok(Self {
            device,
            queue,
            kernel,
            frames: Frames::default(),
        })
    }

    fn scale(&mut self, f: &Video, size: (u32, u32)) -> Result<Video> {
        let clock = std::time::Instant::now();
        let out = self.drawn(f, size)?;
        crate::debug!(
            "lanczos",
            "{}x{} to {}x{} in {:.1} ms",
            f.width(),
            f.height(),
            size.0,
            size.1,
            clock.elapsed().as_secs_f64() * 1e3
        );
        Ok(out)
    }

    fn drawn(&mut self, f: &Video, (w, h): (u32, u32)) -> Result<Video> {
        let layout = crate::hw::sw_format(f);
        let formats = planes(layout)?;
        let out = self
            .frames
            .fresh(f, ((w, h), ffi::AVPixelFormat::from(layout)), WHAT)?;
        let (from, to) = (surface(f, WHAT)?, surface(&out, WHAT)?);
        if from.plane_count() != 2 || to.plane_count() != 2 {
            return Err(refused("IOSurfaces of other than two planes"));
        }
        let buffer = self
            .queue
            .commandBuffer()
            .ok_or_else(|| refused("no command buffer"))?;
        let mut held = Vec::with_capacity(4);
        for (plane, format) in formats.into_iter().enumerate() {
            let read = MTLTextureUsage::ShaderRead;
            let a = texture(&self.device, &from, (plane, format), read, WHAT)?;
            let usage = MTLTextureUsage::ShaderWrite;
            let b = texture(&self.device, &to, (plane, format), usage, WHAT)?;
            let (sx, sy) = if plane == 0 {
                (0, 0)
            } else {
                chroma_shift(layout)
            };
            let (pw, ph) = ((w >> sx) as usize, (h >> sy) as usize);
            let transform = MPSScaleTransform {
                scaleX: pw as f64 / a.width() as f64,
                scaleY: ph as f64 / a.height() as f64,
                translateX: 0.0,
                translateY: 0.0,
            };
            let clip = MTLRegion {
                origin: MTLOrigin { x: 0, y: 0, z: 0 },
                size: MTLSize {
                    width: pw,
                    height: ph,
                    depth: 1,
                },
            };
            // SAFETY: two live textures of this device, which `held` keeps until the work ends; the transform lives through the call that copies it.
            unsafe {
                self.kernel.setScaleTransform(&transform);
                self.kernel.setClipRect(clip);
                self.kernel
                    .encodeToCommandBuffer_sourceTexture_destinationTexture(&buffer, &a, &b);
                self.kernel.setScaleTransform(std::ptr::null());
            }
            held.extend([a, b]);
        }
        buffer.commit();
        buffer.waitUntilCompleted();
        drop(held);
        crate::metal::ended(&buffer, WHAT)?;
        Ok(out)
    }
}
