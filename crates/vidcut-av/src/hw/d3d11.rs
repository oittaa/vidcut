use std::ffi::c_void;

use ::windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_QUERY_DESC,
    D3D11_QUERY_EVENT, D3D11_RESOURCE_MISC_SHARED, D3D11_RESOURCE_MISC_SHARED_NTHANDLE,
    ID3D11Device, ID3D11DeviceContext, ID3D11Query, ID3D11Texture2D,
};
use ::windows::Win32::Graphics::Dxgi::{DXGI_ADAPTER_DESC, IDXGIDevice};
use ::windows::core::Interface;

use super::*;

pub(super) unsafe fn desc(device: &Device) -> Option<DXGI_ADAPTER_DESC> {
    // SAFETY: a D3D11VA device's `hwctx` is an `AVD3D11VADeviceContext`, its device alive with it.
    unsafe {
        let ctx = (*device.0).data as *const ffi::AVHWDeviceContext;
        let hw = (*ctx).hwctx as *const ffi::AVD3D11VADeviceContext;
        let raw = (*hw).device as *mut c_void;
        let d3d11 = ID3D11Device::from_raw_borrowed(&raw)?;
        let dxgi: IDXGIDevice = d3d11.cast().ok()?;
        dxgi.GetAdapter().ok()?.GetDesc().ok()
    }
}

#[repr(C)]
pub(crate) struct DeviceContext {
    pub(crate) device: *mut c_void,
    pub(crate) device_context: *mut c_void,
    video_device: *mut c_void,
    video_context: *mut c_void,
    lock: Option<unsafe extern "C" fn(*mut c_void)>,
    unlock: Option<unsafe extern "C" fn(*mut c_void)>,
    lock_ctx: *mut c_void,
}

#[repr(C)]
struct FramesContext {
    texture: *mut c_void,
    bind_flags: u32,
    misc_flags: u32,
    texture_infos: *mut c_void,
}

pub(crate) unsafe fn device_of(frames: *mut ffi::AVBufferRef) -> *const DeviceContext {
    // SAFETY: a D3D11 frames context's device `hwctx` is an `AVD3D11VADeviceContext`.
    unsafe {
        let device = (*((*frames).data as *const ffi::AVHWFramesContext)).device_ctx;
        (*device).hwctx as *const DeviceContext
    }
}

struct Locked {
    unlock: Option<unsafe extern "C" fn(*mut c_void)>,
    lock_ctx: *mut c_void,
}

impl Drop for Locked {
    fn drop(&mut self) {
        // SAFETY: locked in `locked`, by the same device context.
        if let Some(unlock) = self.unlock {
            unsafe { unlock(self.lock_ctx) };
        }
    }
}

unsafe fn locked<T>(dev: &DeviceContext, work: impl FnOnce() -> T) -> T {
    // SAFETY: the device's own lock; `Locked` unlocks on panic or return.
    unsafe {
        if let Some(lock) = dev.lock {
            lock(dev.lock_ctx);
        }
        let _held = Locked {
            unlock: dev.unlock,
            lock_ctx: dev.lock_ctx,
        };
        work()
    }
}

unsafe fn texture(frame: &frame::Video) -> Result<(ID3D11Texture2D, u32)> {
    // SAFETY: a D3D11 frame's data are its texture and index.
    unsafe {
        let p = (*frame.as_ptr()).data[0] as *mut c_void;
        let index = (*frame.as_ptr()).data[1] as usize as u32;
        ID3D11Texture2D::from_raw_borrowed(&p)
            .map(|t| (t.clone(), index))
            .ok_or_else(|| Error::Invalid("a Direct3D 11 frame without a texture".into()))
    }
}

unsafe fn own_frames(
    source: *mut ffi::AVBufferRef,
    size: (u32, u32),
    flags: (i32, i32),
    what: &str,
) -> Result<*mut ffi::AVBufferRef> {
    // SAFETY: a live frames context's device and layout.
    unsafe {
        let src = (*source).data as *const ffi::AVHWFramesContext;
        frames_on((*src).device_ref, (*src).sw_format, size, flags, what)
    }
}

pub(crate) unsafe fn frames_on(
    device: *mut ffi::AVBufferRef,
    layout: ffi::AVPixelFormat,
    (w, h): (u32, u32),
    (bind_flags, misc_flags): (i32, i32),
    what: &str,
) -> Result<*mut ffi::AVBufferRef> {
    let (w, h) = crate::cuda::frame_sides(w, h)?;
    // SAFETY: frames made on the live D3D11 `device` have an `AVD3D11VAFramesContext` as `hwctx`.
    unsafe {
        let mut frames = ffi::av_hwframe_ctx_alloc(device);
        if frames.is_null() {
            return Err(Error::Invalid("no memory for frames".into()));
        }
        let fc = (*frames).data as *mut ffi::AVHWFramesContext;
        (*fc).format = ffi::AVPixelFormat::AV_PIX_FMT_D3D11;
        (*fc).sw_format = layout;
        (*fc).width = w;
        (*fc).height = h;
        (*fc).initial_pool_size = 0;
        let hw = (*fc).hwctx as *mut FramesContext;
        (*hw).bind_flags = bind_flags as u32;
        (*hw).misc_flags = misc_flags as u32;
        let ret = ffi::av_hwframe_ctx_init(frames);
        if ret < 0 {
            ffi::av_buffer_unref(&mut frames);
            return Err(Error::Ffmpeg {
                context: format!("making Direct3D 11 frames {what}"),
                source: ffmpeg::Error::from(ret),
            });
        }
        Ok(frames)
    }
}

pub(crate) unsafe fn encoder_frames(
    device: *mut ffi::AVBufferRef,
    layout: ffi::AVPixelFormat,
    size: (u32, u32),
) -> Result<*mut ffi::AVBufferRef> {
    let flags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0, 0);
    // SAFETY: as the caller says.
    unsafe { frames_on(device, layout, size, flags, "for the encoder") }
}

unsafe fn pooled(frames: *mut ffi::AVBufferRef) -> Result<frame::Video> {
    let mut out = frame::Video::empty();
    // SAFETY: a fresh frame, given a buffer of the pool.
    let ret = unsafe { ffi::av_hwframe_get_buffer(frames, out.as_mut_ptr(), 0) };
    if ret < 0 {
        return Err(Error::Ffmpeg {
            context: "getting a Direct3D 11 texture".into(),
            source: ffmpeg::Error::from(ret),
        });
    }
    Ok(out)
}

pub(crate) struct Keeper {
    size: (u32, u32),
    source: *mut ffi::AVBufferRef,
    done: ID3D11Query,
    pub(super) like: super::KeptLike,
}

// SAFETY: `source` is reference-counted, and `done` is used only under the device's lock.
unsafe impl Send for Keeper {}

impl Drop for Keeper {
    fn drop(&mut self) {
        // SAFETY: its own reference.
        unsafe { ffi::av_buffer_unref(&mut self.source) };
    }
}

const COPY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

const SHAREABLE: (i32, i32) = (
    D3D11_BIND_SHADER_RESOURCE.0,
    D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0,
);

fn even(f: &frame::Video) -> (u32, u32) {
    (
        f.width().next_multiple_of(2),
        f.height().next_multiple_of(2),
    )
}

unsafe fn event_query(dev: &DeviceContext) -> Result<ID3D11Query> {
    // SAFETY: the device of a live frames context.
    unsafe {
        let device = ID3D11Device::from_raw_borrowed(&dev.device);
        let query = device.and_then(|d| {
            let desc = D3D11_QUERY_DESC {
                Query: D3D11_QUERY_EVENT,
                MiscFlags: 0,
            };
            let mut query = None;
            d.CreateQuery(&desc, Some(&mut query)).ok().and(query)
        });
        query.ok_or_else(|| Error::Invalid("no Direct3D 11 query".into()))
    }
}

unsafe fn copy_picture(
    dev: &DeviceContext,
    done: &ID3D11Query,
    out: &frame::Video,
    f: &frame::Video,
) -> Result<()> {
    // SAFETY: both frames' textures are on `dev`, whose context is used under its lock.
    unsafe {
        let context = ID3D11DeviceContext::from_raw_borrowed(&dev.device_context)
            .ok_or_else(|| Error::Invalid("no Direct3D 11 context".into()))?;
        let ((dst, di), (src, si)) = (texture(out)?, texture(f)?);
        let (right, bottom) = even(f);
        let region = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right,
            bottom,
            back: 1,
        };
        locked(dev, || {
            context.CopySubresourceRegion(&dst, di, 0, 0, 0, &src, si, Some(&region));
            context.End(done);
            context.Flush();
        });
        let started = std::time::Instant::now();
        loop {
            let mut finished = ::windows::core::BOOL(0);
            let mut asked = Ok(());
            locked(dev, || {
                asked = context.GetData(
                    done,
                    Some((&raw mut finished).cast()),
                    size_of::<::windows::core::BOOL>() as u32,
                    0,
                );
            });
            if let Err(e) = asked {
                return Err(Error::Invalid(format!(
                    "waiting for a Direct3D 11 copy: {e}"
                )));
            }
            if finished.as_bool() {
                return Ok(());
            }
            if started.elapsed() > COPY_DEADLINE {
                return Err(Error::Invalid("a Direct3D 11 copy didn't finish".into()));
            }
            std::thread::sleep(std::time::Duration::from_micros(100));
        }
    }
}

impl Keeper {
    pub(super) fn new(
        f: &frame::Video,
        source: *mut ffi::AVBufferRef,
        like: super::KeptLike,
    ) -> Result<Self> {
        // SAFETY: the frame's own frames context, on its device.
        unsafe {
            let done = event_query(&*device_of(source))?;
            let source = ffi::av_buffer_ref(source);
            if source.is_null() {
                return Err(Error::Invalid("no memory for frames".into()));
            }
            Ok(Self {
                size: even(f),
                source,
                done,
                like,
            })
        }
    }

    pub(super) fn copy(&mut self, f: &frame::Video) -> Result<frame::Video> {
        // SAFETY: `out` holds its frames' only reference; both frames are on `source`'s device.
        unsafe {
            let mut frames = own_frames(self.source, self.size, SHAREABLE, "to keep")?;
            let out = pooled(frames);
            ffi::av_buffer_unref(&mut frames);
            let mut out = out?;
            copy_picture(&*device_of(self.source), &self.done, &out, f)?;
            same_picture(&mut out, f);
            (*out.as_mut_ptr()).width = f.width() as i32;
            (*out.as_mut_ptr()).height = f.height() as i32;
            Ok(out)
        }
    }
}

pub struct Shared {
    texture: frame::Video,
    done: ID3D11Query,
    like: (usize, format::Pixel, (u32, u32)),
}

// SAFETY: `texture` is reference-counted, and `done` is used only under the device's lock.
unsafe impl Send for Shared {}

fn like_of(f: &frame::Video) -> Result<(usize, format::Pixel, (u32, u32))> {
    let (frames, _) = super::frames_of(f)?;
    // SAFETY: a live frames context's device.
    let device = unsafe { (*((*frames).data as *const ffi::AVHWFramesContext)).device_ctx };
    Ok((device as usize, sw_format(f), even(f)))
}

impl Shared {
    pub fn new(f: &frame::Video) -> Result<Self> {
        let like = like_of(f)?;
        let (source, _) = super::frames_of(f)?;
        // SAFETY: the frame's own frames context, on its device.
        unsafe {
            let done = event_query(&*device_of(source))?;
            let mut frames = own_frames(source, like.2, SHAREABLE, "to share")?;
            let texture = pooled(frames);
            ffi::av_buffer_unref(&mut frames);
            Ok(Self {
                texture: texture?,
                done,
                like,
            })
        }
    }

    pub fn fits(&self, f: &frame::Video) -> bool {
        like_of(f).is_ok_and(|like| like == self.like)
    }

    pub fn texture(&self) -> *mut c_void {
        // SAFETY: a D3D11 frame's first data pointer is its texture.
        unsafe { (*self.texture.as_ptr()).data[0] as *mut c_void }
    }

    pub fn fill(&mut self, f: &frame::Video) -> Result<()> {
        if !self.fits(f) {
            return Err(Error::Invalid(
                "a picture of another size, layout or device".into(),
            ));
        }
        let (frames, _) = super::frames_of(f)?;
        // SAFETY: both frames are on the device of `f`'s frames context.
        unsafe { copy_picture(&*device_of(frames), &self.done, &self.texture, f) }
    }
}

fn same_picture(to: &mut frame::Video, from: &frame::Video) {
    // SAFETY: both frames are valid; plain fields.
    unsafe {
        let (o, i) = (to.as_mut_ptr(), from.as_ptr());
        (*o).pts = (*i).pts;
        (*o).color_range = (*i).color_range;
        (*o).colorspace = (*i).colorspace;
        (*o).color_primaries = (*i).color_primaries;
        (*o).color_trc = (*i).color_trc;
        (*o).chroma_location = (*i).chroma_location;
        (*o).sample_aspect_ratio = (*i).sample_aspect_ratio;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frames_reject_a_side_past_i32() {
        // SAFETY: the size is rejected before the device pointer is used.
        let err = unsafe {
            frames_on(
                std::ptr::null_mut(),
                ffi::AVPixelFormat::AV_PIX_FMT_NV12,
                (2_147_483_648, 480),
                (0, 0),
                "test",
            )
        }
        .unwrap_err();
        assert_eq!(err.to_string(), "2147483648×480 doesn't fit a frame");
    }
}
