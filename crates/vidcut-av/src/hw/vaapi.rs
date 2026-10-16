//! VA-API's frames: Intel's surfaces mapped for the copy down, the filters
//! that shrink and fit frames on the GPU, and the DRM export the viewer
//! imports on Linux.

use super::*;

/// A VA-API frame of Intel's driver copied down the quicker way on its
/// display ([`Ways`]); `None` for any other frame.
pub(super) fn intel_download(src: &frame::Video) -> Option<Result<frame::Video>> {
    // SAFETY: a valid frame.
    let display = unsafe { intel_va(src) }?;
    Some(quicker_down(src, display))
}

/// `src` copied down the quicker way on Intel's VA-API `display`.
fn quicker_down(src: &frame::Video, display: usize) -> Result<frame::Video> {
    if Ways::maps(display) {
        let started = std::time::Instant::now();
        match mapped_down(src)? {
            Some(f) => {
                Ways::took(display, Way::Mapped, started.elapsed(), &f);
                return Ok(f);
            }
            None => Ways::unmapped(display),
        }
    }
    let started = std::time::Instant::now();
    let f = fetched(src)?;
    Ways::took(display, Way::Fetched, started.elapsed(), &f);
    Ok(f)
}

/// The ways Intel's VA-API frames come down.
#[derive(Clone, Copy)]
enum Way {
    /// The surface mapped itself (`mapped_down`).
    Mapped,
    /// FFmpeg's copy of it (`fetched`).
    Fetched,
}

/// Which way Intel's frames come down faster on a VA-API display, timed on
/// its first frames, each way in turn. Mapped, an N150's take half the time
/// (gotchas.md); FFmpeg finds Gen7–Gen9's mapped memory uncached, so slow
/// to read, and a card of its own reads its memory across the bus.
#[derive(Clone, Copy)]
struct Ways {
    /// The display it is for.
    display: usize,
    /// Each way's quickest time a megabyte, in seconds, and its frames
    /// timed (mapping that failed counts as never quicker).
    mapped: (f64, u32),
    fetched: (f64, u32),
}

/// The frames timed each way before one is kept.
const WAY_TRIALS: u32 = 4;

/// The displays' [`Ways`].
static WAYS: Mutex<Vec<Ways>> = Mutex::new(Vec::new());

impl Ways {
    /// Whether the next frame on `display` is mapped: in turn while they
    /// are timed, then the quicker way.
    fn maps(display: usize) -> bool {
        let ways = WAYS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(w) = ways.iter().find(|w| w.display == display) else {
            return true;
        };
        if w.mapped.1 < WAY_TRIALS || w.fetched.1 < WAY_TRIALS {
            w.mapped.1 <= w.fetched.1
        } else {
            w.mapped.0 <= w.fetched.0
        }
    }

    /// `display`'s, made at the first frame.
    fn of(ways: &mut Vec<Ways>, display: usize) -> &mut Ways {
        let i = match ways.iter().position(|w| w.display == display) {
            Some(i) => i,
            None => {
                ways.push(Ways {
                    display,
                    mapped: (f64::INFINITY, 0),
                    fetched: (f64::INFINITY, 0),
                });
                ways.len() - 1
            }
        };
        &mut ways[i]
    }

    /// A frame `f` came down `way` on `display` in `took`.
    fn took(display: usize, way: Way, took: std::time::Duration, f: &frame::Video) {
        let mb = (0..f.planes()).map(|p| f.data(p).len()).sum::<usize>() as f64 / 1e6;
        let per_mb = took.as_secs_f64() / mb.max(1e-6);
        let mut ways = WAYS.lock().unwrap_or_else(|e| e.into_inner());
        let w = Self::of(&mut ways, display);
        let slot = match way {
            Way::Mapped => &mut w.mapped,
            Way::Fetched => &mut w.fetched,
        };
        *slot = (slot.0.min(per_mb), slot.1.saturating_add(1));
    }

    /// A frame on `display` wouldn't map: its frames come down FFmpeg's
    /// way from now on.
    fn unmapped(display: usize) {
        let mut ways = WAYS.lock().unwrap_or_else(|e| e.into_inner());
        Self::of(&mut ways, display).mapped = (f64::INFINITY, WAY_TRIALS);
    }
}

/// `src` copied out of its surface mapped for reading itself
/// (`vaDeriveImage`), where FFmpeg's copy-down reads a copy the driver makes
/// of it (`vaGetImage`): on an Intel N150 in half the time (gotchas.md).
/// `None` when it doesn't map.
fn mapped_down(src: &frame::Video) -> Result<Option<frame::Video>> {
    let mut map = frame::Video::empty();
    let flags = ffi::AV_HWFRAME_MAP_READ as i32 | ffi::AV_HWFRAME_MAP_DIRECT as i32;
    // SAFETY: a fresh frame, mapped in the frames' own layout; unmapped when
    // it drops.
    unsafe {
        (*map.as_mut_ptr()).format = ffi::AVPixelFormat::AV_PIX_FMT_NONE as i32;
        if ffi::av_hwframe_map(map.as_mut_ptr(), src.as_ptr(), flags) < 0 {
            return Ok(None);
        }
    }
    let mut dst = frame::Video::new(map.format(), src.width(), src.height());
    // SAFETY: both frames are valid, of one layout, the mapped one no
    // smaller.
    if unsafe { ffi::av_frame_copy(dst.as_mut_ptr(), map.as_ptr()) } < 0 {
        return Ok(None);
    }
    props(&mut dst, src)?;
    Ok(Some(dst))
}

/// The display of `frame` when it is a VA-API frame of Intel's driver
/// (iHD), by the vendor string libva gives the display.
///
/// # Safety
/// `frame` is a valid frame.
#[cfg(target_os = "linux")]
unsafe fn intel_va(frame: &frame::Video) -> Option<usize> {
    use std::ffi::{CStr, c_char, c_void};
    /// `AVVAAPIDeviceContext`.
    #[repr(C)]
    struct VaapiDevice {
        display: *mut c_void,
        driver_quirks: std::ffi::c_uint,
    }
    type Query = unsafe extern "C" fn(*mut c_void) -> *const c_char;
    // libva's, loaded with FFmpeg: looked up rather than linked, so a
    // build of FFmpeg without VA-API needs no libva.
    static QUERY: std::sync::OnceLock<Option<Query>> = std::sync::OnceLock::new();
    let query = QUERY.get_or_init(|| {
        // SAFETY: a symbol of the process's, which is libva's function or
        // nothing.
        unsafe {
            let f = libc::dlsym(libc::RTLD_DEFAULT, c"vaQueryVendorString".as_ptr());
            (!f.is_null()).then(|| std::mem::transmute::<*mut c_void, Query>(f))
        }
    });
    let query = (*query)?;
    // SAFETY: a hardware frame's frames context holds its device's, and a
    // VA-API device's context is an AVVAAPIDeviceContext with its display.
    unsafe {
        let frames = (*frame.as_ptr()).hw_frames_ctx;
        if frames.is_null() {
            return None;
        }
        let device = (*((*frames).data as *const ffi::AVHWFramesContext)).device_ctx;
        if device.is_null() || (*device).type_ != ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI {
            return None;
        }
        let va = (*device).hwctx as *const VaapiDevice;
        if va.is_null() || (*va).display.is_null() {
            return None;
        }
        let vendor = query((*va).display);
        (!vendor.is_null() && CStr::from_ptr(vendor).to_bytes().starts_with(b"Intel iHD"))
            .then_some((*va).display as usize)
    }
}

/// VA-API is Linux's.
///
/// # Safety
/// None.
#[cfg(not(target_os = "linux"))]
unsafe fn intel_va(_: &frame::Video) -> Option<usize> {
    None
}

/// A pool VA-API's decoders share ([`share_frames`]): what it is for, and
/// the frames.
#[cfg(target_os = "linux")]
struct Shared {
    device: usize,
    codec: ffi::AVCodecID,
    layout: ffi::AVPixelFormat,
    size: (i32, i32),
    frames: *mut ffi::AVBufferRef,
}
// SAFETY: frames contexts are reference-counted and may be used on any
// thread.
#[cfg(target_os = "linux")]
unsafe impl Send for Shared {}

/// The pools VA-API's decoders share.
#[cfg(target_os = "linux")]
static SHARED: Mutex<Vec<Shared>> = Mutex::new(Vec::new());

/// Lets the shared pools go that nothing else holds: no decoder takes from
/// them, and no frame of theirs is left. Each frees all its surfaces.
#[cfg(target_os = "linux")]
fn let_unused_go(shared: &mut Vec<Shared>) {
    shared.retain_mut(|s| {
        // SAFETY: the pool's own reference.
        unsafe {
            let unused = ffi::av_buffer_get_ref_count(s.frames) == 1;
            if unused {
                ffi::av_buffer_unref(&mut s.frames);
            }
            !unused
        }
    });
}

/// The shared pool whose surface `f` is, where it is one's: its memory goes
/// only when all of the pool's frames do.
#[cfg(target_os = "linux")]
pub(super) fn pooled(f: &frame::Video) -> Option<usize> {
    // SAFETY: reads the frame's own field.
    let frames = unsafe { (*f.as_ptr()).hw_frames_ctx };
    if frames.is_null() {
        return None;
    }
    let shared = SHARED.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: live buffer references, compared by their frames contexts.
    let found = shared
        .iter()
        .any(|s| unsafe { (*s.frames).data == (*frames).data });
    // SAFETY: as above.
    found.then(|| unsafe { (*frames).data as usize })
}

/// Lets the shared pools go that nothing uses any more ([`let_unused_go`]),
/// as pictures are kept: a pool goes soon after its last picture does, not
/// when the next decoder opens.
#[cfg(target_os = "linux")]
pub(crate) fn let_unused_frames_go() {
    let_unused_go(&mut SHARED.lock().unwrap_or_else(|e| e.into_inner()));
}

/// Gives a VA-API decoder, in its `get_format`, the frames every decoder on
/// its device shares for its codec, layout and size. A decoder's frames come
/// from a pool that frees nothing while any of its frames is kept
/// (gotchas.md): a decoder made anew (proxies switched, a cut to another
/// file and back, FFmpeg 8.0's H.264 decoder at every seek) would start a
/// pool beside the one the kept pictures hold. Sharing it, the new decoder
/// takes the surfaces those pictures gave back. Where the decoder's pool
/// has a fixed size (VA-API before 1.0) or anything fails, it makes its own.
///
/// # Safety
/// `ctx` is a decoder on a VA-API device, in its `get_format`.
#[cfg(target_os = "linux")]
pub(crate) unsafe fn share_frames(ctx: *mut ffi::AVCodecContext) {
    // SAFETY: the decoder's own device; the frames FFmpeg describes for it
    // are ours until set up or let go, and a shared one is given by a
    // reference of its own.
    unsafe {
        let mut frames = ptr::null_mut();
        let ret = ffi::avcodec_get_hw_frames_parameters(
            ctx,
            (*ctx).hw_device_ctx,
            ffi::AVPixelFormat::AV_PIX_FMT_VAAPI,
            &mut frames,
        );
        if ret < 0 || frames.is_null() {
            return;
        }
        let fc = (*frames).data as *const ffi::AVHWFramesContext;
        if (*fc).initial_pool_size != 0 {
            ffi::av_buffer_unref(&mut frames);
            return;
        }
        let device = (*(*ctx).hw_device_ctx).data as usize;
        let (codec, layout, size) = (
            (*ctx).codec_id,
            (*fc).sw_format,
            ((*fc).width, (*fc).height),
        );
        let mut shared = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        let_unused_go(&mut shared);
        let found = shared.iter().find(|s| {
            s.device == device && s.codec == codec && s.layout == layout && s.size == size
        });
        let pool = match found {
            Some(s) => {
                ffi::av_buffer_unref(&mut frames);
                s.frames
            }
            None => {
                if ffi::av_hwframe_ctx_init(frames) < 0 {
                    ffi::av_buffer_unref(&mut frames);
                    return;
                }
                shared.push(Shared {
                    device,
                    codec,
                    layout,
                    size,
                    frames,
                });
                frames
            }
        };
        ffi::av_buffer_unref(&mut (*ctx).hw_frames_ctx);
        (*ctx).hw_frames_ctx = ffi::av_buffer_ref(pool);
    }
}

/// VA-API's kept frames copied into surfaces of their own by the driver's
/// `vaCopy`, where it has one (Intel's: an exact copy): freed with the
/// picture, where a shared pool keeps its peak while any of its frames is
/// kept. Where the driver has none (Mesa's, gotchas.md), frames are kept
/// as they come, from the shared pool ([`share_frames`]). Each copy runs
/// while the next frame decodes: the frame it reads, and the copy, are held
/// until the next is kept, and waited for then, so the decoder doesn't
/// decode into a surface the copy still reads.
#[cfg(target_os = "linux")]
pub(crate) struct Keeper {
    va: &'static Va,
    display: *mut std::ffi::c_void,
    /// The decoder's device.
    device: *mut ffi::AVBufferRef,
    like: Like,
    /// The frame the last copy reads, and the copy.
    pending: Option<(frame::Video, frame::Video)>,
}

// SAFETY: libva's display may be used from any thread, and the device and
// frames are reference-counted.
#[cfg(target_os = "linux")]
unsafe impl Send for Keeper {}

#[cfg(target_os = "linux")]
impl Drop for Keeper {
    fn drop(&mut self) {
        self.settle();
        // SAFETY: its own reference.
        unsafe { ffi::av_buffer_unref(&mut self.device) };
    }
}

/// What a keeper is made for: the decoder's device, and its frames' layout
/// and size (their surfaces', which a copy's must be).
#[cfg(target_os = "linux")]
type Like = (usize, format::Pixel, i32, i32);

/// `VACopyObject` for a surface.
#[cfg(target_os = "linux")]
#[repr(C)]
struct CopyObject {
    /// `VACopyObjectSurface`.
    kind: u32,
    surface: u32,
    va_reserved: [u32; 8],
}

/// libva's calls the keeper makes, as `va.h` declares them: libva's, loaded
/// with FFmpeg, looked up rather than linked, so a build of FFmpeg without
/// VA-API needs no libva. `vaCopy` came with libva 2.12.
#[cfg(target_os = "linux")]
struct Va {
    copy: CopyFn,
    sync_surface: SyncFn,
}

#[cfg(target_os = "linux")]
type CopyFn =
    unsafe extern "C" fn(*mut std::ffi::c_void, *mut CopyObject, *mut CopyObject, u32) -> i32;
#[cfg(target_os = "linux")]
type SyncFn = unsafe extern "C" fn(*mut std::ffi::c_void, u32) -> i32;

/// `VACopyOption` with `va_copy_sync` `VA_EXEC_ASYNC`: the call returns
/// once the copy is queued.
#[cfg(target_os = "linux")]
const COPY_ASYNC: u32 = 1;

/// `VA_STATUS_ERROR_UNIMPLEMENTED`: a driver without `vaCopy`.
#[cfg(target_os = "linux")]
const UNIMPLEMENTED: i32 = 0x14;

/// The displays whose driver has no `vaCopy`.
#[cfg(target_os = "linux")]
static NO_COPY: Mutex<Vec<usize>> = Mutex::new(Vec::new());

#[cfg(target_os = "linux")]
impl Va {
    fn get() -> Option<&'static Va> {
        static VA: std::sync::OnceLock<Option<Va>> = std::sync::OnceLock::new();
        VA.get_or_init(|| {
            // SAFETY: the process's symbols, libva's functions or nothing,
            // taken as `va.h` declares them.
            unsafe {
                let copy = libc::dlsym(libc::RTLD_DEFAULT, c"vaCopy".as_ptr());
                let sync = libc::dlsym(libc::RTLD_DEFAULT, c"vaSyncSurface".as_ptr());
                (!copy.is_null() && !sync.is_null()).then(|| Va {
                    copy: std::mem::transmute::<*mut std::ffi::c_void, CopyFn>(copy),
                    sync_surface: std::mem::transmute::<*mut std::ffi::c_void, SyncFn>(sync),
                })
            }
        })
        .as_ref()
    }
}

/// `f`'s VA-API surface.
#[cfg(target_os = "linux")]
fn surface(f: &frame::Video) -> u32 {
    // SAFETY: a VA-API frame's surface is in its fourth data pointer.
    unsafe { (*f.as_ptr()).data[3] as usize as u32 }
}

/// The device reference of `f`'s frames, its display, and what a keeper
/// for it is made for.
#[cfg(target_os = "linux")]
fn like_of(f: &frame::Video) -> Result<(*mut ffi::AVBufferRef, *mut std::ffi::c_void, Like)> {
    let (frames, _) = frames_of(f)?;
    // SAFETY: a live VA-API frames context; its device's context is an
    // AVVAAPIDeviceContext, its display first.
    unsafe {
        let fc = (*frames).data as *const ffi::AVHWFramesContext;
        let device = (*fc).device_ref;
        let display = *((*(*fc).device_ctx).hwctx as *const *mut std::ffi::c_void);
        let like = (
            (*device).data as usize,
            format::Pixel::from((*fc).sw_format),
            (*fc).width,
            (*fc).height,
        );
        Ok((device, display, like))
    }
}

#[cfg(target_os = "linux")]
impl Keeper {
    /// A keeper for frames like `f`, a VA-API frame; `None` where its
    /// driver has no `vaCopy`, and its frames are kept as they come.
    pub(crate) fn new(f: &frame::Video) -> Result<Option<Self>> {
        let (device, display, like) = like_of(f)?;
        let copies = !NO_COPY
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&(display as usize));
        let Some(va) = Va::get().filter(|_| copies) else {
            return Ok(None);
        };
        // SAFETY: a live device reference.
        let device = unsafe { ffi::av_buffer_ref(device) };
        if device.is_null() {
            return Err(Error::Invalid("no memory for a device reference".into()));
        }
        Ok(Some(Self {
            va,
            display,
            device,
            like,
            pending: None,
        }))
    }

    /// Whether it keeps frames like `f`.
    pub(crate) fn is_for(&self, f: &frame::Video) -> bool {
        like_of(f).is_ok_and(|(_, _, like)| like == self.like)
    }

    /// Waits for the copy before, and lets go of the frame it read.
    fn settle(&mut self) {
        if let Some((_, copy)) = self.pending.take() {
            // SAFETY: the copy's surface, held until now.
            unsafe { (self.va.sync_surface)(self.display, surface(&copy)) };
        }
    }

    /// `f`'s picture in a surface of its own, copied by the driver while
    /// the next frame decodes; `None` where the driver has no `vaCopy`.
    /// Its properties are the caller's to copy.
    pub(crate) fn copy(&mut self, f: &frame::Video) -> Result<Option<frame::Video>> {
        self.settle();
        let mut out = frame::Video::empty();
        // SAFETY: frames of one surface on the decoder's device, in its
        // frames' layout and size; the frame taken from them holds the only
        // reference left, so the surface goes with it.
        unsafe {
            let mut frames = ffi::av_hwframe_ctx_alloc(self.device);
            if frames.is_null() {
                return Err(Error::Invalid("no memory for frames".into()));
            }
            let fc = (*frames).data as *mut ffi::AVHWFramesContext;
            (*fc).format = ffi::AVPixelFormat::AV_PIX_FMT_VAAPI;
            (*fc).sw_format = self.like.1.into();
            (*fc).width = self.like.2;
            (*fc).height = self.like.3;
            (*fc).initial_pool_size = 1;
            let mut ret = ffi::av_hwframe_ctx_init(frames);
            if ret >= 0 {
                ret = ffi::av_hwframe_get_buffer(frames, out.as_mut_ptr(), 0);
            }
            ffi::av_buffer_unref(&mut frames);
            if ret < 0 {
                return Err(Error::Ffmpeg {
                    context: "making a VA-API surface to keep".into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
            (*out.as_mut_ptr()).width = f.width() as i32;
            (*out.as_mut_ptr()).height = f.height() as i32;
        }
        let object = |f: &frame::Video| CopyObject {
            kind: 0,
            surface: surface(f),
            va_reserved: [0; 8],
        };
        let (mut to, mut from) = (object(&out), object(f));
        // SAFETY: two surfaces of the display, of one layout and size.
        let status = unsafe { (self.va.copy)(self.display, &mut to, &mut from, COPY_ASYNC) };
        if status == UNIMPLEMENTED {
            NO_COPY
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(self.display as usize);
            return Ok(None);
        }
        if status != 0 {
            return Err(Error::Invalid(format!(
                "VA-API: copying a kept frame failed (status {status})"
            )));
        }
        self.pending = Some((
            crate::export::video::frame_ref(f)?,
            crate::export::video::frame_ref(&out)?,
        ));
        Ok(Some(out))
    }
}

/// The filter that shrinks VA-API's frames to `w` by `h`.
pub(super) fn shrink_spec((w, h): (u32, u32)) -> String {
    format!("scale_vaapi=w={w}:h={h}")
}

/// Whether VA-API fits a frame into the output as `ops` says
/// ([`super::Fitter`]): where its colors stay as they are, since
/// `scale_vaapi` only resamples them.
pub(crate) fn can_fit(ops: &crate::route::Ops) -> bool {
    ops.colors_kept
}

/// The filters that fit VA-API's frames into a `w` by `h` picture of
/// `layout` (NV12, P010): turned clockwise by `rotation`, scaled to
/// `fitted` and centered on black at even offsets where it is smaller.
pub(super) fn fit_spec(
    rotation: u16,
    fitted: (u32, u32),
    (w, h): (u32, u32),
    layout: format::Pixel,
) -> String {
    let (fw, fh) = fitted;
    let layout = if layout == format::Pixel::P010LE {
        "p010"
    } else {
        "nv12"
    };
    // As the CPU's `transpose` and flips turn them.
    let turn = match rotation {
        90 => "transpose_vaapi=dir=clock,",
        180 => "transpose_vaapi=dir=reversal,",
        270 => "transpose_vaapi=dir=cclock,",
        _ => "",
    };
    let mut spec = format!("{turn}scale_vaapi=w={fw}:h={fh}:format={layout}");
    if fitted != (w, h) {
        let (x, y) = (((w - fw) / 2) & !1, ((h - fh) / 2) & !1);
        spec.push_str(&format!(",pad_vaapi=w={w}:h={h}:x={x}:y={y}:color=black"));
    }
    spec
}

/// A VA-API frame's surface for another device to read: its planes as
/// DMA-BUF files with their layouts and DRM format modifier
/// (`AV_PIX_FMT_DRM_PRIME`), mapped once the GPU's work on it is done.
/// Linux.
#[cfg(target_os = "linux")]
pub struct DrmFrame(frame::Video);

/// One plane of a [`DrmFrame`].
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
pub struct DrmPlane<'a> {
    /// The file holding it, the frame's.
    pub fd: std::os::fd::BorrowedFd<'a>,
    pub offset: u64,
    /// Bytes from a row to the next.
    pub pitch: u64,
    /// How it is laid out in memory (tiled, compressed...).
    pub modifier: u64,
}

#[cfg(target_os = "linux")]
mod drm {
    //! `libavutil/hwcontext_drm.h`.
    pub(super) const MAX_PLANES: usize = 4;

    #[repr(C)]
    pub(super) struct Object {
        pub(super) fd: std::ffi::c_int,
        pub(super) size: usize,
        pub(super) format_modifier: u64,
    }

    #[repr(C)]
    pub(super) struct Plane {
        pub(super) object_index: std::ffi::c_int,
        pub(super) offset: isize,
        pub(super) pitch: isize,
    }

    #[repr(C)]
    pub(super) struct Layer {
        pub(super) format: u32,
        pub(super) nb_planes: std::ffi::c_int,
        pub(super) planes: [Plane; MAX_PLANES],
    }

    #[repr(C)]
    pub(super) struct Frame {
        pub(super) nb_objects: std::ffi::c_int,
        pub(super) objects: [Object; MAX_PLANES],
        pub(super) nb_layers: std::ffi::c_int,
        pub(super) layers: [Layer; MAX_PLANES],
    }
}

#[cfg(target_os = "linux")]
impl DrmFrame {
    /// `f`, a VA-API frame, mapped for reading: FFmpeg waits for the
    /// surface's decoding (or copy) to be done, and exports a layer for each
    /// plane.
    pub fn export(f: &frame::Video) -> Result<Self> {
        let mut map = frame::Video::empty();
        // SAFETY: a fresh frame, mapped as DRM PRIME; unmapped when it drops.
        let ret = unsafe {
            (*map.as_mut_ptr()).format = ffi::AVPixelFormat::AV_PIX_FMT_DRM_PRIME as i32;
            ffi::av_hwframe_map(
                map.as_mut_ptr(),
                f.as_ptr(),
                ffi::AV_HWFRAME_MAP_READ as i32,
            )
        };
        if ret < 0 {
            return Err(Error::Ffmpeg {
                context: "exporting a VA-API surface".into(),
                source: ffmpeg::Error::from(ret),
            });
        }
        Ok(Self(map))
    }

    /// Its planes: luma, then chroma (each layer's first).
    pub fn planes(&self) -> Vec<DrmPlane<'_>> {
        // SAFETY: a DRM PRIME frame's first data pointer is its descriptor,
        // which lives as long as the mapping; its files are open as long.
        unsafe {
            let d = &*((*self.0.as_ptr()).data[0] as *const drm::Frame);
            let layers = d.nb_layers.clamp(0, drm::MAX_PLANES as i32) as usize;
            d.layers[..layers]
                .iter()
                .filter(|l| l.nb_planes > 0)
                .filter_map(|l| {
                    let p = &l.planes[0];
                    let o = d.objects.get(usize::try_from(p.object_index).ok()?)?;
                    Some(DrmPlane {
                        fd: std::os::fd::BorrowedFd::borrow_raw(o.fd),
                        offset: p.offset as u64,
                        pitch: p.pitch as u64,
                        modifier: o.format_modifier,
                    })
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn a_copy_object_is_laid_out_as_va_h_has_it() {
        assert_eq!(std::mem::size_of::<CopyObject>(), 40);
    }
    use crate::route::Ops;

    /// An upright frame into NV12 pictures in its own colors.
    fn kept() -> Ops {
        Ops {
            rotation: 0,
            layout: format::Pixel::NV12,
            out_layout: format::Pixel::NV12,
            rgb: false,
            colors_kept: true,
            pads: false,
            into: false,
        }
    }

    #[test]
    fn frames_in_the_outputs_colors_fit_on_the_gpu() {
        assert!(can_fit(&kept()));
    }

    #[test]
    fn frames_whose_colors_change_dont_fit_on_the_gpu() {
        assert!(!can_fit(&Ops {
            colors_kept: false,
            ..kept()
        }));
    }

    #[test]
    fn shrinking_scales_to_the_size() {
        assert_eq!(shrink_spec((960, 540)), "scale_vaapi=w=960:h=540");
    }

    #[test]
    fn fitting_at_the_output_size_adds_no_padding() {
        assert_eq!(
            fit_spec(0, (1920, 1080), (1920, 1080), format::Pixel::NV12),
            "scale_vaapi=w=1920:h=1080:format=nv12"
        );
    }

    #[test]
    fn fitting_a_turned_frame_centers_it_on_black() {
        assert_eq!(
            fit_spec(90, (608, 1080), (1920, 1080), format::Pixel::P010LE),
            "transpose_vaapi=dir=clock,scale_vaapi=w=608:h=1080:format=p010,pad_vaapi=w=1920:h=1080:x=656:y=0:color=black"
        );
    }
}
