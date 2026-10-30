use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::CFRetained;
use objc2_core_video::CVPixelBufferGetIOSurface;
use objc2_io_surface::IOSurfaceRef;
use objc2_metal::{
    MTLCommandBuffer, MTLCommandBufferStatus, MTLCommandQueue, MTLCreateSystemDefaultDevice,
    MTLDevice, MTLPixelFormat, MTLTexture, MTLTextureDescriptor, MTLTextureType, MTLTextureUsage,
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
    let (w, h) = match surface.plane_count() {
        0 => (surface.width(), surface.height()),
        _ => (
            surface.width_of_plane(plane),
            surface.height_of_plane(plane),
        ),
    };
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

pub(crate) fn imported(
    device: &wgpu::Device,
    surface: &IOSurfaceRef,
    (plane, mtl, format): (usize, MTLPixelFormat, wgpu::TextureFormat),
    (usage, wanted, uses): (MTLTextureUsage, wgpu::TextureUsages, wgpu::TextureUses),
    what: &str,
) -> Result<wgpu::Texture> {
    use wgpu::hal::api::Metal;
    // SAFETY: wgpu's own device, which makes textures here only.
    let hal = unsafe { device.as_hal::<Metal>() }.ok_or_else(|| refused(what, "not Metal"))?;
    let raw = texture(hal.raw_device(), surface, (plane, mtl), usage, what)?;
    let (width, height) = (raw.width() as u32, raw.height() as u32);
    let extent = wgpu::hal::CopyExtent {
        width,
        height,
        depth: 1,
    };
    // SAFETY: `raw` is a texture of this device, as described, used only as `uses` says.
    Ok(unsafe {
        let raw = wgpu::hal::metal::Device::texture_from_raw(
            raw,
            format,
            MTLTextureType::Type2D,
            1,
            1,
            extent,
            None,
        );
        device.create_texture_from_hal::<Metal>(
            raw,
            &wgpu::TextureDescriptor {
                label: Some(what),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wanted,
                view_formats: &[],
            },
            uses,
        )
    })
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
