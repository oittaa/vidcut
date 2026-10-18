use std::ffi::c_void;
use std::mem::ManuallyDrop;

use ::windows::Win32::Foundation::RECT;
use ::windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_DECODER, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX,
    D3D11_QUERY_DESC, D3D11_QUERY_EVENT, D3D11_RESOURCE_MISC_SHARED,
    D3D11_RESOURCE_MISC_SHARED_NTHANDLE, D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_VIDEO_COLOR,
    D3D11_VIDEO_COLOR_0, D3D11_VIDEO_COLOR_RGBA, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
    D3D11_VIDEO_PROCESSOR_CAPS, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_FEATURE_CAPS_ROTATION, D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT,
    D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_ROTATION_90,
    D3D11_VIDEO_PROCESSOR_ROTATION_180, D3D11_VIDEO_PROCESSOR_ROTATION_270,
    D3D11_VIDEO_PROCESSOR_ROTATION_IDENTITY, D3D11_VIDEO_PROCESSOR_STREAM,
    D3D11_VIDEO_USAGE_PLAYBACK_NORMAL, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D, ID3D11Device, ID3D11DeviceContext, ID3D11Query,
    ID3D11Texture2D, ID3D11VideoContext1, ID3D11VideoDevice, ID3D11VideoProcessor,
    ID3D11VideoProcessorEnumerator,
};
use ::windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_TYPE, DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P601,
    DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P709, DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P2020,
    DXGI_COLOR_SPACE_YCBCR_FULL_GHLG_TOPLEFT_P2020, DXGI_COLOR_SPACE_YCBCR_STUDIO_G2084_LEFT_P2020,
    DXGI_FORMAT_NV12, DXGI_FORMAT_P010,
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

unsafe fn locked<T>(dev: &DeviceContext, work: impl FnOnce() -> T) -> T {
    // SAFETY: the device's own lock.
    unsafe {
        if let Some(lock) = dev.lock {
            lock(dev.lock_ctx);
        }
        let done = work();
        if let Some(unlock) = dev.unlock {
            unlock(dev.lock_ctx);
        }
        done
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

pub(crate) fn can_fit(ops: &crate::route::Ops) -> bool {
    ops.colors_kept
        && ops.into
        && !(ops.layout == format::Pixel::NV12 && ops.out_layout == format::Pixel::P010LE)
}

pub(crate) struct Scaler {
    halvings: Vec<Scaler>,
    frames: *mut ffi::AVBufferRef,
    video: ID3D11VideoDevice,
    context: ID3D11VideoContext1,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    size: (u32, u32),
    place: (u32, u32, u32, u32),
    rotation: u16,
    space: DXGI_COLOR_SPACE_TYPE,
}

impl Drop for Scaler {
    fn drop(&mut self) {
        // SAFETY: its own reference.
        unsafe { ffi::av_buffer_unref(&mut self.frames) };
    }
}

impl Scaler {
    pub(super) fn new(
        f: &frame::Video,
        source: *mut ffi::AVBufferRef,
        size: (u32, u32),
    ) -> Result<Self> {
        // SAFETY: the frame's own frames context.
        let layout = unsafe { (*((*source).data as *const ffi::AVHWFramesContext)).sw_format };
        let (halvings, mut scaler) = Self::passes(f, source, layout, size, 0)?;
        scaler.halvings = halvings;
        Ok(scaler)
    }

    pub(super) unsafe fn into(
        f: &frame::Video,
        source: *mut ffi::AVBufferRef,
        frames: *mut ffi::AVBufferRef,
        fitted: (u32, u32),
        rotation: u16,
    ) -> Result<Self> {
        // SAFETY: a live frames context, the encoder's.
        let (layout, size) = unsafe {
            let fc = &*((*frames).data as *const ffi::AVHWFramesContext);
            (fc.sw_format, (fc.width as u32, fc.height as u32))
        };
        let (halvings, check) = Self::passes(f, source, layout, fitted, rotation)?;
        drop(check);
        let (fw, fh) = fitted;
        let place = (((size.0 - fw) / 2) & !1, ((size.1 - fh) / 2) & !1, fw, fh);
        // SAFETY: another reference to the encoder's frames.
        let frames = unsafe { ffi::av_buffer_ref(frames) };
        if frames.is_null() {
            return Err(Error::Invalid("no memory for frames".into()));
        }
        let (from, input) = halvings
            .last()
            .map_or((source, (f.width(), f.height())), |h| (h.frames, h.size));
        let mut scaler = Self::made(from, input, frames, size, place, color_space(f))?;
        scaler.halvings = halvings;
        scaler.turn(rotation)?;
        Ok(scaler)
    }

    fn passes(
        f: &frame::Video,
        source: *mut ffi::AVBufferRef,
        layout: ffi::AVPixelFormat,
        fitted: (u32, u32),
        rotation: u16,
    ) -> Result<(Vec<Self>, Self)> {
        let (input, space) = ((f.width(), f.height()), color_space(f));
        if let Some(one) = Self::tested(input, space, source, layout, fitted, rotation)? {
            return Ok((Vec::new(), one));
        }
        let sizes = halving_sizes(input, stored(fitted, rotation));
        if sizes.is_empty() || !readable(fitted) {
            return Err(Error::Invalid(
                "the video processor changes the levels".into(),
            ));
        }
        let changed =
            || Error::Invalid("the video processor changes the levels in halvings too".into());
        // SAFETY: the frame's own frames context.
        let own = unsafe { (*((*source).data as *const ffi::AVHWFramesContext)).sw_format };
        let mut halvings: Vec<Self> = Vec::new();
        let mut at = input;
        for half in sizes {
            let from = halvings.last().map_or(source, |h| h.frames);
            halvings.push(Self::tested(at, space, from, own, half, 0)?.ok_or_else(changed)?);
            at = half;
        }
        let from = halvings.last().map_or(source, |h| h.frames);
        let last = Self::tested(at, space, from, layout, fitted, rotation)?.ok_or_else(changed)?;
        crate::debug!(
            "decoding",
            "{}x{} frames shrink to {}x{} in {} passes on the video processor, as one pass changes their levels",
            input.0,
            input.1,
            fitted.0,
            fitted.1,
            halvings.len() + 1
        );
        Ok((halvings, last))
    }

    fn tested(
        input: (u32, u32),
        space: DXGI_COLOR_SPACE_TYPE,
        source: *mut ffi::AVBufferRef,
        layout: ffi::AVPixelFormat,
        size: (u32, u32),
        rotation: u16,
    ) -> Result<Option<Self>> {
        let flags = (D3D11_BIND_RENDER_TARGET.0, 0);
        // SAFETY: the device of a live frames context.
        let frames = unsafe {
            let device = (*((*source).data as *const ffi::AVHWFramesContext)).device_ref;
            frames_on(device, layout, size, flags, "to scale to")?
        };
        let place = (0, 0, size.0, size.1);
        let mut scaler = Self::made(source, input, frames, size, place, space)?;
        scaler.turn(rotation)?;
        let keeps = scaler.keeps_levels(source, input)?;
        Ok(keeps.then_some(scaler))
    }

    fn turn(&mut self, rotation: u16) -> Result<()> {
        if rotation != 0 {
            let mut caps = D3D11_VIDEO_PROCESSOR_CAPS::default();
            // SAFETY: the processor's enumerator fills the caps.
            let turns = unsafe { self.enumerator.GetVideoProcessorCaps(&mut caps) }.is_ok()
                && caps.FeatureCaps & D3D11_VIDEO_PROCESSOR_FEATURE_CAPS_ROTATION.0 as u32 != 0;
            if !turns {
                return Err(Error::Invalid(
                    "the video processor doesn't turn pictures".into(),
                ));
            }
        }
        self.rotation = rotation;
        Ok(())
    }

    fn made(
        source: *mut ffi::AVBufferRef,
        input: (u32, u32),
        mut frames: *mut ffi::AVBufferRef,
        size: (u32, u32),
        place: (u32, u32, u32, u32),
        space: DXGI_COLOR_SPACE_TYPE,
    ) -> Result<Self> {
        let failed = |what: &'static str| {
            move |e: ::windows::core::Error| Error::Invalid(format!("{what}: {e}"))
        };
        let layout = |frames: *mut ffi::AVBufferRef| {
            // SAFETY: a live frames context.
            match format::Pixel::from(unsafe {
                (*((*frames).data as *const ffi::AVHWFramesContext)).sw_format
            }) {
                format::Pixel::NV12 => Ok(DXGI_FORMAT_NV12),
                format::Pixel::P010LE => Ok(DXGI_FORMAT_P010),
                other => Err(Error::Invalid(format!(
                    "no scaling {other:?} on Direct3D 11"
                ))),
            }
        };
        // SAFETY: the device of a live frames context.
        let made = (|| unsafe {
            let (from, to) = (layout(source)?, layout(frames)?);
            let dev = &*device_of(source);
            let none = || Error::Invalid("no Direct3D 11 device".into());
            let device = ID3D11Device::from_raw_borrowed(&dev.device).ok_or_else(none)?;
            let immediate =
                ID3D11DeviceContext::from_raw_borrowed(&dev.device_context).ok_or_else(none)?;
            let video: ID3D11VideoDevice = device
                .cast()
                .map_err(failed("no Direct3D 11 video device"))?;
            let context: ID3D11VideoContext1 = immediate
                .cast()
                .map_err(failed("no Direct3D 11 video context"))?;
            let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputWidth: input.0,
                InputHeight: input.1,
                OutputWidth: size.0,
                OutputHeight: size.1,
                Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
                ..Default::default()
            };
            let enumerator = video
                .CreateVideoProcessorEnumerator(&desc)
                .map_err(failed("no video processor"))?;
            let takes = |layout, how: i32| {
                enumerator
                    .CheckVideoProcessorFormat(layout)
                    .is_ok_and(|t| t & how as u32 != 0)
            };
            if !takes(from, D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT.0)
                || !takes(to, D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT.0)
            {
                return Err(Error::Invalid(format!(
                    "the video processor doesn't scale {from:?} into {to:?}"
                )));
            }
            let processor = video
                .CreateVideoProcessor(&enumerator, 0)
                .map_err(failed("no video processor"))?;
            Ok((video, context, enumerator, processor))
        })();
        match made {
            Ok((video, context, enumerator, processor)) => Ok(Self {
                halvings: Vec::new(),
                frames,
                video,
                context,
                enumerator,
                processor,
                size,
                place,
                rotation: 0,
                space,
            }),
            Err(e) => {
                // SAFETY: the reference it was given.
                unsafe { ffi::av_buffer_unref(&mut frames) };
                Err(e)
            }
        }
    }

    fn keeps_levels(&mut self, source: *mut ffi::AVBufferRef, input: (u32, u32)) -> Result<bool> {
        if !readable(self.size) {
            return Ok(false);
        }
        let (ow, oh) = (self.size.0 as usize, self.size.1 as usize);
        let layout = |frames: *mut ffi::AVBufferRef| {
            // SAFETY: a live frames context.
            format::Pixel::from(unsafe {
                (*((*frames).data as *const ffi::AVHWFramesContext)).sw_format
            })
        };
        let size = (input.0.next_multiple_of(2), input.1.next_multiple_of(2));
        let picture = test_picture(layout(source), size, self.rotation);
        let bands = BANDS.len();
        // SAFETY: `frame` holds its own reference to the test frames once `frames` is let go.
        let uploaded = unsafe {
            let flags = (D3D11_BIND_DECODER.0, 0);
            let mut frames = own_frames(source, size, flags, "to test with")?;
            let frame = pooled(frames);
            ffi::av_buffer_unref(&mut frames);
            let mut frame = frame?;
            let ret = ffi::av_hwframe_transfer_data(frame.as_mut_ptr(), picture.as_ptr(), 0);
            if ret < 0 {
                return Err(Error::Ffmpeg {
                    context: "uploading a test picture".into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
            frame
        };
        let got = downloaded(&self.scale(&uploaded)?)?;
        let wide_out = layout(self.frames) == format::Pixel::P010LE;
        let read = |plane: usize, x: usize, y: usize| {
            let (stride, data) = (got.stride(plane), got.data(plane));
            if wide_out {
                u16::from_le_bytes([data[y * stride + 2 * x], data[y * stride + 2 * x + 1]]) >> 6
            } else {
                u16::from(data[y * stride + x])
            }
        };
        let (scale, near) = if wide_out { (4, 4) } else { (1, 1) };
        let checked = if full_range(self.space) {
            bands
        } else {
            bands - OUTSIDE
        };
        Ok(BANDS[..checked].iter().enumerate().all(|(k, [y, u, v])| {
            let middle = (2 * k + 1) * ow / (2 * bands);
            let luma = (oh / 4..oh * 3 / 4).all(|row| {
                (middle - 1..middle + 1).all(|x| read(0, x, row).abs_diff(y * scale) <= near)
            });
            let chroma = (oh / 8..oh * 3 / 8).all(|row| {
                (middle / 2 - 1..middle / 2 + 1).all(|x| {
                    read(1, 2 * x, row).abs_diff(u * scale) <= near
                        && read(1, 2 * x + 1, row).abs_diff(v * scale) <= near
                })
            });
            luma && chroma
        }))
    }

    pub(super) fn scale(&mut self, f: &frame::Video) -> Result<frame::Video> {
        let mut halved = None;
        for pass in &mut self.halvings {
            halved = Some(pass.draw(halved.as_ref().unwrap_or(f))?);
        }
        self.draw(halved.as_ref().unwrap_or(f))
    }

    fn draw(&mut self, f: &frame::Video) -> Result<frame::Video> {
        let (w, h) = self.size;
        let rect = |x: u32, y: u32, w: u32, h: u32| RECT {
            left: x as i32,
            top: y as i32,
            right: (x + w) as i32,
            bottom: (y + h) as i32,
        };
        let failed = |what: &'static str| {
            move |e: ::windows::core::Error| Error::Invalid(format!("{what}: {e}"))
        };
        // SAFETY: the views are of `f`'s and `out`'s textures, used under the device's lock.
        unsafe {
            let out = pooled(self.frames)?;
            let ((src, index), (dst, _)) = (texture(f)?, texture(&out)?);
            let dev = &*device_of(self.frames);
            locked(dev, || -> Result<()> {
                let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                    FourCC: 0,
                    ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                        Texture2D: D3D11_TEX2D_VPIV {
                            MipSlice: 0,
                            ArraySlice: index,
                        },
                    },
                };
                let mut input = None;
                self.video
                    .CreateVideoProcessorInputView(&src, &self.enumerator, &desc, Some(&mut input))
                    .map_err(failed("viewing a decoded frame"))?;
                let desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                        Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                    },
                };
                let mut output = None;
                self.video
                    .CreateVideoProcessorOutputView(
                        &dst,
                        &self.enumerator,
                        &desc,
                        Some(&mut output),
                    )
                    .map_err(failed("viewing a texture to scale to"))?;
                let output = output
                    .ok_or_else(|| Error::Invalid("no view of a texture to scale to".into()))?;
                let (c, p) = (&self.context, &self.processor);
                c.VideoProcessorSetStreamColorSpace1(p, 0, self.space);
                c.VideoProcessorSetOutputColorSpace1(p, self.space);
                c.VideoProcessorSetStreamAutoProcessingMode(p, 0, false);
                let rotation = match self.rotation {
                    90 => D3D11_VIDEO_PROCESSOR_ROTATION_90,
                    180 => D3D11_VIDEO_PROCESSOR_ROTATION_180,
                    270 => D3D11_VIDEO_PROCESSOR_ROTATION_270,
                    _ => D3D11_VIDEO_PROCESSOR_ROTATION_IDENTITY,
                };
                c.VideoProcessorSetStreamRotation(p, 0, self.rotation != 0, rotation);
                c.VideoProcessorSetStreamSourceRect(
                    p,
                    0,
                    true,
                    Some(&rect(0, 0, f.width(), f.height())),
                );
                let (x, y, pw, ph) = self.place;
                c.VideoProcessorSetStreamDestRect(p, 0, true, Some(&rect(x, y, pw, ph)));
                c.VideoProcessorSetOutputTargetRect(p, true, Some(&rect(0, 0, w, h)));
                let level = if full_range(self.space) {
                    16.0 / 255.0
                } else {
                    0.0
                };
                let black = D3D11_VIDEO_COLOR {
                    Anonymous: D3D11_VIDEO_COLOR_0 {
                        RGBA: D3D11_VIDEO_COLOR_RGBA {
                            R: level,
                            G: level,
                            B: level,
                            A: 1.0,
                        },
                    },
                };
                c.VideoProcessorSetOutputBackgroundColor(p, false, &black);
                let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
                    Enable: true.into(),
                    pInputSurface: ManuallyDrop::new(input),
                    ..Default::default()
                };
                let blit = c.VideoProcessorBlt(p, &output, 0, std::slice::from_ref(&stream));
                ManuallyDrop::drop(&mut stream.pInputSurface);
                blit.map_err(failed("scaling a frame on the GPU"))
            })?;
            Ok(out)
        }
    }
}

const OUTSIDE: usize = 2;

fn test_picture(pixel: format::Pixel, size: (u32, u32), rotation: u16) -> frame::Video {
    let wide = pixel == format::Pixel::P010LE;
    let shift = if wide { 8 } else { 0 };
    let (w, h) = (size.0 as usize, size.1 as usize);
    let bands = BANDS.len();
    let band = |x: usize, y: usize| match rotation {
        90 => (h - 1 - y) * bands / h,
        180 => (w - 1 - x) * bands / w,
        270 => y * bands / h,
        _ => x * bands / w,
    };
    let mut picture = frame::Video::new(pixel, size.0, size.1);
    let mut put = |plane: usize, x: usize, y: usize, value: u16| {
        let stride = picture.stride(plane);
        let data = picture.data_mut(plane);
        let value = value << shift;
        if wide {
            data[y * stride + 2 * x..][..2].copy_from_slice(&value.to_le_bytes());
        } else {
            data[y * stride + x] = value as u8;
        }
    };
    for y in 0..h {
        for x in 0..w {
            put(0, x, y, BANDS[band(x, y)][0]);
        }
    }
    for y in 0..h / 2 {
        for x in 0..w / 2 {
            let [_, u, v] = BANDS[band(2 * x, 2 * y)];
            put(1, 2 * x, y, u);
            put(1, 2 * x + 1, y, v);
        }
    }
    picture
}

fn stored((w, h): (u32, u32), rotation: u16) -> (u32, u32) {
    if rotation % 180 == 90 { (h, w) } else { (w, h) }
}

fn halving_sizes(from: (u32, u32), to: (u32, u32)) -> Vec<(u32, u32)> {
    let half = |side: u32, to: u32| if side > 2 * to { (side / 2) & !1 } else { side };
    let mut at = from;
    let mut sizes = Vec::new();
    while at.0 > 2 * to.0 || at.1 > 2 * to.1 {
        at = (half(at.0, to.0), half(at.1, to.1));
        sizes.push(at);
    }
    sizes
}

fn color_space(f: &frame::Video) -> DXGI_COLOR_SPACE_TYPE {
    use crate::color::Matrix;
    use vidcut_core::model::Hdr;
    let colors = crate::color::colors_of(f);
    match (colors.hdr, colors.matrix) {
        (Some(Hdr::Pq), _) => DXGI_COLOR_SPACE_YCBCR_STUDIO_G2084_LEFT_P2020,
        (Some(Hdr::Hlg), _) => DXGI_COLOR_SPACE_YCBCR_FULL_GHLG_TOPLEFT_P2020,
        (None, Matrix::Bt2020) => DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P2020,
        (None, Matrix::Bt601) => DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P601,
        (None, Matrix::Bt709) => DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P709,
    }
}

fn full_range(space: DXGI_COLOR_SPACE_TYPE) -> bool {
    space != DXGI_COLOR_SPACE_YCBCR_STUDIO_G2084_LEFT_P2020
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::Ops;

    fn into_frames() -> Ops {
        Ops {
            rotation: 0,
            layout: format::Pixel::NV12,
            out_layout: format::Pixel::NV12,
            rgb: false,
            colors_kept: true,
            pads: false,
            into: true,
        }
    }

    #[test]
    fn frames_fit_into_the_encoders_frames() {
        assert!(can_fit(&into_frames()));
    }

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

    #[test]
    fn frames_dont_fit_without_the_encoders_frames() {
        assert!(!can_fit(&Ops {
            into: false,
            ..into_frames()
        }));
    }

    #[test]
    fn eight_bit_frames_dont_fit_into_ten_bit_ones() {
        assert!(!can_fit(&Ops {
            out_layout: format::Pixel::P010LE,
            ..into_frames()
        }));
    }

    #[test]
    fn a_shrink_to_a_quarter_halves_once() {
        assert_eq!(halving_sizes((3840, 2160), (960, 540)), [(1920, 1080)]);
    }

    #[test]
    fn a_shrink_to_an_eighth_halves_twice() {
        assert_eq!(
            halving_sizes((3840, 2160), (480, 270)),
            [(1920, 1080), (960, 540)]
        );
    }

    #[test]
    fn a_4k_shrink_to_half_doesnt_halve() {
        assert!(halving_sizes((3840, 2160), (1920, 1080)).is_empty());
    }

    #[test]
    fn a_1080p_shrink_to_half_doesnt_halve() {
        assert!(halving_sizes((1920, 1080), (960, 540)).is_empty());
    }

    #[test]
    fn a_side_that_shrinks_at_most_twice_isnt_halved() {
        assert_eq!(halving_sizes((2880, 2160), (1920, 720)), [(2880, 1080)]);
    }

    #[test]
    fn a_4k_picture_turned_into_720x1280_halves_once() {
        let toward = stored((720, 1280), 90);
        assert_eq!(halving_sizes((3840, 2160), toward), [(1920, 1080)]);
    }

    #[test]
    fn a_4k_picture_turned_into_1080x1920_doesnt_halve() {
        let toward = stored((1080, 1920), 90);
        assert!(halving_sizes((3840, 2160), toward).is_empty());
    }

    fn rows(picture: &frame::Video, p: usize, n: usize) -> Vec<Vec<u8>> {
        let stride = picture.stride(p);
        let data = picture.data(p);
        (0..picture.plane_height(p) as usize)
            .map(|y| data[y * stride..][..n].to_vec())
            .collect()
    }

    #[test]
    fn a_test_picture_turned_clockwise_has_its_first_band_at_the_bottom() {
        let picture = test_picture(format::Pixel::NV12, (20, 20), 90);
        let luma: Vec<u8> = rows(&picture, 0, 1).concat();
        assert_eq!(
            luma,
            [
                250, 250, 4, 4, 32, 32, 173, 173, 63, 63, 235, 235, 192, 192, 128, 128, 64, 64, 16,
                16
            ]
        );
    }

    #[test]
    fn a_test_picture_turned_counterclockwise_has_its_first_band_at_the_top() {
        let picture = test_picture(format::Pixel::NV12, (20, 20), 270);
        let luma: Vec<u8> = rows(&picture, 0, 1).concat();
        assert_eq!(
            luma,
            [
                16, 16, 64, 64, 128, 128, 192, 192, 235, 235, 63, 63, 173, 173, 32, 32, 4, 4, 250,
                250
            ]
        );
    }

    #[test]
    fn a_test_picture_turned_clockwise_has_its_colors_turned_too() {
        let picture = test_picture(format::Pixel::NV12, (20, 20), 90);
        assert_eq!(
            rows(&picture, 1, 2),
            [
                [128, 248],
                [8, 128],
                [240, 118],
                [42, 26],
                [102, 240],
                [128, 128],
                [128, 128],
                [128, 128],
                [128, 128],
                [128, 128]
            ]
        );
    }
}
