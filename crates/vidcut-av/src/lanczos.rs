use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::CFRetained;
use objc2_core_video::CVPixelBufferGetIOSurface;
use objc2_io_surface::IOSurfaceRef;
use objc2_metal::{
    MTLClearColor, MTLCommandBuffer, MTLCommandBufferStatus, MTLCommandEncoder, MTLCommandQueue,
    MTLCreateSystemDefaultDevice, MTLDevice, MTLLoadAction, MTLOrigin, MTLPixelFormat, MTLRegion,
    MTLRenderPassDescriptor, MTLSize, MTLStoreAction, MTLTexture, MTLTextureDescriptor,
    MTLTextureUsage,
};
use objc2_metal_performance_shaders::{MPSImageLanczosScale, MPSScaleTransform};

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

fn chroma_shift(layout: format::Pixel) -> (u32, u32) {
    match layout {
        format::Pixel::NV12 | format::Pixel::P010LE => (1, 1),
        format::Pixel::NV16 | format::Pixel::P210LE => (1, 0),
        _ => (0, 0),
    }
}

fn black(format: MTLPixelFormat, full: bool) -> [[f64; 2]; 2] {
    let (luma, chroma) = if matches!(format, MTLPixelFormat::R16Unorm | MTLPixelFormat::RG16Unorm) {
        (4096.0 / 65535.0, 32768.0 / 65535.0)
    } else {
        (16.0 / 255.0, 128.0 / 255.0)
    };
    [[if full { 0.0 } else { luma }, 0.0], [chroma, chroma]]
}

pub(crate) fn centered(f: &Video, out: (u32, u32), size: (u32, u32)) -> (u32, u32) {
    let (sx, sy) = chroma_shift(crate::hw::sw_format(f));
    (
        (out.0.saturating_sub(size.0) / 2) >> sx << sx,
        (out.1.saturating_sub(size.1) / 2) >> sy << sy,
    )
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
        self.place(f, size, ((0, 0), size))
    }

    pub(crate) fn place(
        &mut self,
        f: &Video,
        size: (u32, u32),
        rect: ((u32, u32), (u32, u32)),
    ) -> Result<Video> {
        let clock = std::time::Instant::now();
        let out = self.drawn(f, size, rect)?;
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

    fn drawn(
        &mut self,
        f: &Video,
        size: (u32, u32),
        ((x, y), (w, h)): ((u32, u32), (u32, u32)),
    ) -> Result<Video> {
        let layout = crate::hw::sw_format(f);
        let formats = planes(layout)?;
        let out = self
            .frames
            .fresh(f, (size, ffi::AVPixelFormat::from(layout)), WHAT)?;
        let (from, to) = (surface(f)?, surface(&out)?);
        if from.plane_count() != 2 || to.plane_count() != 2 {
            return Err(refused("IOSurfaces of other than two planes"));
        }
        let padded = (x, y, w, h) != (0, 0, size.0, size.1);
        // SAFETY: a plain field of a valid frame.
        let full = unsafe { (*f.as_ptr()).color_range } == ffi::AVColorRange::AVCOL_RANGE_JPEG;
        let buffer = self
            .queue
            .commandBuffer()
            .ok_or_else(|| refused("no command buffer"))?;
        let mut held = Vec::with_capacity(4);
        for (plane, format) in formats.into_iter().enumerate() {
            let a = self.texture(&from, plane, format, MTLTextureUsage::ShaderRead)?;
            let usage = MTLTextureUsage::ShaderWrite | MTLTextureUsage::RenderTarget;
            let b = self.texture(&to, plane, format, usage)?;
            if padded {
                let [r, g] = black(format, full)[plane];
                let pass = MTLRenderPassDescriptor::renderPassDescriptor();
                // SAFETY: attachment 0 of a new descriptor.
                let color = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
                color.setTexture(Some(&b));
                color.setLoadAction(MTLLoadAction::Clear);
                color.setClearColor(MTLClearColor {
                    red: r,
                    green: g,
                    blue: 0.0,
                    alpha: 1.0,
                });
                color.setStoreAction(MTLStoreAction::Store);
                buffer
                    .renderCommandEncoderWithDescriptor(&pass)
                    .ok_or_else(|| refused("no render pass"))?
                    .endEncoding();
            }
            let (sx, sy) = if plane == 0 {
                (0, 0)
            } else {
                chroma_shift(layout)
            };
            let (px, py, pw, ph) = (
                (x >> sx) as usize,
                (y >> sy) as usize,
                (w >> sx) as usize,
                (h >> sy) as usize,
            );
            let transform = MPSScaleTransform {
                scaleX: pw as f64 / a.width() as f64,
                scaleY: ph as f64 / a.height() as f64,
                translateX: 0.0,
                translateY: 0.0,
            };
            let clip = MTLRegion {
                origin: MTLOrigin { x: px, y: py, z: 0 },
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
        if buffer.status() != MTLCommandBufferStatus::Completed {
            return Err(buffer.error().map_or_else(
                || refused("the scaling did not end"),
                |e| crate::frame_processor::failed(WHAT, e),
            ));
        }
        Ok(out)
    }
}
