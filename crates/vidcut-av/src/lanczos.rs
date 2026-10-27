use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::CFRetained;
use objc2_core_video::CVPixelBufferGetIOSurface;
use objc2_io_surface::IOSurfaceRef;
use objc2_metal::{
    MTLCommandBuffer, MTLCommandBufferStatus, MTLCommandQueue, MTLCreateSystemDefaultDevice,
    MTLDevice, MTLPixelFormat, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};
use objc2_metal_performance_shaders::MPSImageLanczosScale;

use crate::error::{Error, Result};
use crate::ffmpeg::{ffi, format, frame::Video};
use crate::frame_processor::{Frames, pixels};

const WHAT: &str = "lanczos scaling on Metal";

pub(crate) struct Lanczos {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
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
        format::Pixel::NV12 | format::Pixel::NV16 => {
            Ok([MTLPixelFormat::R8Unorm, MTLPixelFormat::RG8Unorm])
        }
        format::Pixel::P010LE | format::Pixel::P210LE => {
            Ok([MTLPixelFormat::R16Unorm, MTLPixelFormat::RG16Unorm])
        }
        other => Err(refused(&format!("no planes for {other:?}"))),
    }
}

fn surface(f: &Video) -> Result<CFRetained<IOSurfaceRef>> {
    CVPixelBufferGetIOSurface(Some(pixels(f, WHAT)?))
        .ok_or_else(|| refused("a pixel buffer without an IOSurface"))
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
        let device = MTLCreateSystemDefaultDevice().ok_or_else(|| refused("no Metal device"))?;
        let queue = device
            .newCommandQueue()
            .ok_or_else(|| refused("no command queue"))?;
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

    pub(crate) fn scale(&mut self, f: &Video, size: (u32, u32)) -> Result<Video> {
        let clock = std::time::Instant::now();
        let out = self.scaled(f, size)?;
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

    fn scaled(&mut self, f: &Video, size: (u32, u32)) -> Result<Video> {
        let layout = crate::hw::sw_format(f);
        let formats = planes(layout)?;
        let out = self
            .frames
            .fresh(f, (size, ffi::AVPixelFormat::from(layout)), WHAT)?;
        let (from, to) = (surface(f)?, surface(&out)?);
        if from.plane_count() != 2 || to.plane_count() != 2 {
            return Err(refused("IOSurfaces of other than two planes"));
        }
        let buffer = self
            .queue
            .commandBuffer()
            .ok_or_else(|| refused("no command buffer"))?;
        let mut held = Vec::with_capacity(4);
        for (plane, format) in formats.into_iter().enumerate() {
            let a = self.texture(&from, plane, format, MTLTextureUsage::ShaderRead)?;
            let b = self.texture(&to, plane, format, MTLTextureUsage::ShaderWrite)?;
            // SAFETY: two live textures of this device, which `held` keeps until the work ends.
            unsafe {
                self.kernel
                    .encodeToCommandBuffer_sourceTexture_destinationTexture(&buffer, &a, &b)
            };
            held.extend([a, b]);
        }
        buffer.commit();
        buffer.waitUntilCompleted();
        drop(held);
        if buffer.status() != MTLCommandBufferStatus::Completed {
            return Err(buffer.error().map_or_else(
                || refused("the scaling did not end"),
                |e| crate::frame_processor::failed(WHAT, e),
            ));
        }
        Ok(out)
    }
}
