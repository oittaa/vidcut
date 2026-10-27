use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::CFRetained;
use objc2_core_video::CVPixelBufferGetIOSurface;
use objc2_io_surface::IOSurfaceRef;
use objc2_metal::{
    MTLCommandBuffer, MTLCommandBufferStatus, MTLCommandQueue, MTLCreateSystemDefaultDevice,
    MTLDevice, MTLPixelFormat, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};

use crate::error::{Error, Result};
use crate::ffmpeg::frame::Video;
use crate::frame_processor::{failed, pixels};

pub(crate) type Device = Retained<ProtocolObject<dyn MTLDevice>>;
pub(crate) type Queue = Retained<ProtocolObject<dyn MTLCommandQueue>>;
pub(crate) type Texture = Retained<ProtocolObject<dyn MTLTexture>>;

fn refused(what: &str, why: &str) -> Error {
    Error::Invalid(format!("{what}: {why}"))
}

pub(crate) fn device(what: &str) -> Result<(Device, Queue)> {
    let device = MTLCreateSystemDefaultDevice().ok_or_else(|| refused(what, "no Metal device"))?;
    let queue = device
        .newCommandQueue()
        .ok_or_else(|| refused(what, "no command queue"))?;
    Ok((device, queue))
}

pub(crate) fn surface(f: &Video, what: &str) -> Result<CFRetained<IOSurfaceRef>> {
    CVPixelBufferGetIOSurface(Some(pixels(f, what)?))
        .ok_or_else(|| refused(what, "a pixel buffer without an IOSurface"))
}

pub(crate) fn texture(
    device: &Device,
    surface: &IOSurfaceRef,
    (plane, format): (usize, MTLPixelFormat),
    usage: MTLTextureUsage,
    what: &str,
) -> Result<Texture> {
    let (w, h) = (
        surface.width_of_plane(plane),
        surface.height_of_plane(plane),
    );
    // SAFETY: `desc` is the plane's size and format; Metal checks it against `surface`.
    unsafe {
        let desc = MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            format, w, h, false,
        );
        desc.setUsage(usage);
        device.newTextureWithDescriptor_iosurface_plane(&desc, surface, plane)
    }
    .ok_or_else(|| refused(what, "Metal won't make a texture of the IOSurface"))
}

pub(crate) fn ended(buffer: &ProtocolObject<dyn MTLCommandBuffer>, what: &str) -> Result<()> {
    if buffer.status() == MTLCommandBufferStatus::Completed {
        return Ok(());
    }
    Err(buffer.error().map_or_else(
        || refused(what, "the work did not end"),
        |e| failed(what, e),
    ))
}
