use super::*;

pub(super) fn intel_download(src: &frame::Video) -> Option<Result<frame::Video>> {
    // SAFETY: a valid frame.
    let display = unsafe { intel_va(src) }?;
    Some(quicker_down(src, display))
}

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

#[derive(Clone, Copy)]
enum Way {
    Mapped,
    Fetched,
}

#[derive(Clone, Copy)]
struct Ways {
    display: usize,
    mapped: (f64, u32),
    fetched: (f64, u32),
}

const WAY_TRIALS: u32 = 4;

static WAYS: Mutex<Vec<Ways>> = Mutex::new(Vec::new());

impl Ways {
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

    fn unmapped(display: usize) {
        let mut ways = WAYS.lock().unwrap_or_else(|e| e.into_inner());
        Self::of(&mut ways, display).mapped = (f64::INFINITY, WAY_TRIALS);
    }
}

fn mapped_down(src: &frame::Video) -> Result<Option<frame::Video>> {
    let mut map = frame::Video::empty();
    let flags = ffi::AV_HWFRAME_MAP_READ as i32 | ffi::AV_HWFRAME_MAP_DIRECT as i32;
    // SAFETY: `map` is a fresh frame, mapped from `src` in its own layout, unmapped when it drops.
    unsafe {
        (*map.as_mut_ptr()).format = ffi::AVPixelFormat::AV_PIX_FMT_NONE as i32;
        if ffi::av_hwframe_map(map.as_mut_ptr(), src.as_ptr(), flags) < 0 {
            return Ok(None);
        }
    }
    let mut dst = frame::Video::new(map.format(), src.width(), src.height());
    // SAFETY: `dst` is allocated in `map`'s layout, at a size the mapping is no smaller than.
    if unsafe { ffi::av_frame_copy(dst.as_mut_ptr(), map.as_ptr()) } < 0 {
        return Ok(None);
    }
    props(&mut dst, src)?;
    Ok(Some(dst))
}

#[cfg(target_os = "linux")]
unsafe fn intel_va(frame: &frame::Video) -> Option<usize> {
    use std::ffi::{CStr, c_char, c_void};
    #[repr(C)]
    struct VaapiDevice {
        display: *mut c_void,
        driver_quirks: std::ffi::c_uint,
    }
    type Query = unsafe extern "C" fn(*mut c_void) -> *const c_char;
    static QUERY: std::sync::OnceLock<Option<Query>> = std::sync::OnceLock::new();
    let query = QUERY.get_or_init(|| {
        // SAFETY: the process's `vaQueryVendorString`, if any, is libva's, as `Query` types it.
        unsafe {
            let f = libc::dlsym(libc::RTLD_DEFAULT, c"vaQueryVendorString".as_ptr());
            (!f.is_null()).then(|| std::mem::transmute::<*mut c_void, Query>(f))
        }
    });
    let query = (*query)?;
    // SAFETY: `frame` holds its frames context; a VA-API `hwctx` is an `AVVAAPIDeviceContext`.
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

#[cfg(not(target_os = "linux"))]
unsafe fn intel_va(_: &frame::Video) -> Option<usize> {
    None
}

#[cfg(target_os = "linux")]
struct Shared {
    device: usize,
    codec: ffi::AVCodecID,
    layout: ffi::AVPixelFormat,
    size: (i32, i32),
    frames: *mut ffi::AVBufferRef,
}
// SAFETY: `frames` is a reference-counted frames context, which any thread may use.
#[cfg(target_os = "linux")]
unsafe impl Send for Shared {}

#[cfg(target_os = "linux")]
static SHARED: Mutex<Vec<Shared>> = Mutex::new(Vec::new());

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

#[cfg(target_os = "linux")]
pub(crate) fn let_unused_frames_go() {
    let_unused_go(&mut SHARED.lock().unwrap_or_else(|e| e.into_inner()));
}

#[cfg(target_os = "linux")]
pub(crate) unsafe fn share_frames(ctx: *mut ffi::AVCodecContext) {
    // SAFETY: `ctx` is a VA-API decoder; `frames` is ours; `ctx` gets the pool by a new reference.
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

#[cfg(target_os = "linux")]
pub(crate) struct Keeper {
    va: &'static Va,
    display: *mut std::ffi::c_void,
    device: *mut ffi::AVBufferRef,
    like: Like,
    pending: Option<(frame::Video, frame::Video)>,
}

// SAFETY: libva's display is usable from any thread; the device and frames are reference-counted.
#[cfg(target_os = "linux")]
unsafe impl Send for Keeper {}

#[cfg(target_os = "linux")]
impl Drop for Keeper {
    fn drop(&mut self) {
        if self.settle().is_err() {
            std::mem::forget(self.pending.take());
        }
        // SAFETY: its own reference.
        unsafe { ffi::av_buffer_unref(&mut self.device) };
    }
}

#[cfg(target_os = "linux")]
type Like = (usize, format::Pixel, i32, i32);

#[cfg(target_os = "linux")]
#[repr(C)]
struct CopyObject {
    kind: u32,
    surface: u32,
    va_reserved: [u32; 8],
}

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

#[cfg(target_os = "linux")]
const COPY_ASYNC: u32 = 1;

#[cfg(target_os = "linux")]
const UNIMPLEMENTED: i32 = 0x14;

#[cfg(target_os = "linux")]
static NO_COPY: Mutex<Vec<usize>> = Mutex::new(Vec::new());

#[cfg(target_os = "linux")]
impl Va {
    fn get() -> Option<&'static Va> {
        static VA: std::sync::OnceLock<Option<Va>> = std::sync::OnceLock::new();
        VA.get_or_init(|| {
            // SAFETY: any `vaCopy` and `vaSyncSurface` are libva's, typed as `va.h` declares them.
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

#[cfg(target_os = "linux")]
fn surface(f: &frame::Video) -> u32 {
    // SAFETY: a VA-API frame's surface is in its fourth data pointer.
    unsafe { (*f.as_ptr()).data[3] as usize as u32 }
}

#[cfg(target_os = "linux")]
fn like_of(f: &frame::Video) -> Result<(*mut ffi::AVBufferRef, *mut std::ffi::c_void, Like)> {
    let (frames, _) = frames_of(f)?;
    // SAFETY: `frames` is a live VA-API frames context; its device `hwctx` starts with the display.
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

    pub(crate) fn is_for(&self, f: &frame::Video) -> bool {
        like_of(f).is_ok_and(|(_, _, like)| like == self.like)
    }

    fn settle(&mut self) -> Result<()> {
        let Some((_, copy)) = &self.pending else {
            return Ok(());
        };
        // SAFETY: the copy's surface, held until now.
        let status = unsafe { (self.va.sync_surface)(self.display, surface(copy)) };
        if status != 0 {
            return Err(Error::Invalid(format!(
                "VA-API: waiting for a kept frame's copy failed (status {status})"
            )));
        }
        self.pending = None;
        Ok(())
    }

    pub(crate) fn copy(&mut self, f: &frame::Video) -> Result<Option<frame::Video>> {
        self.settle()?;
        let mut out = frame::Video::empty();
        // SAFETY: `self.device` is a live VA-API device; `out` holds its own reference to `frames`.
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
        handed_back(self.settle(), out).map(Some)
    }
}

#[cfg(target_os = "linux")]
fn handed_back<T, E>(settled: std::result::Result<(), E>, frame: T) -> std::result::Result<T, E> {
    settled?;
    Ok(frame)
}

#[cfg(target_os = "linux")]
pub(super) fn surface_bytes(f: &frame::Video) -> Option<usize> {
    type Kind = (usize, ffi::AVPixelFormat, i32, i32);
    static SIZES: Mutex<Vec<(Kind, usize)>> = Mutex::new(Vec::new());
    let (frames, _) = frames_of(f).ok()?;
    // SAFETY: a live VA-API frames context.
    let kind = unsafe {
        let fc = (*frames).data as *const ffi::AVHWFramesContext;
        (
            (*(*fc).device_ref).data as usize,
            (*fc).sw_format,
            (*fc).width,
            (*fc).height,
        )
    };
    let mut sizes = SIZES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, bytes)) = sizes.iter().find(|(k, _)| *k == kind) {
        return Some(*bytes);
    }
    let exported = DrmFrame::export(f).ok()?;
    // SAFETY: the DRM PRIME mapping's `data[0]` is its descriptor, alive as long as `exported`.
    let bytes = unsafe {
        let d = &*((*exported.0.as_ptr()).data[0] as *const drm::Frame);
        let objects = d.nb_objects.clamp(0, drm::MAX_PLANES as i32) as usize;
        d.objects[..objects].iter().map(|o| o.size).sum::<usize>()
    };
    (bytes > 0).then(|| {
        sizes.push((kind, bytes));
        bytes
    })
}

pub(super) fn shrink_spec((w, h): (u32, u32)) -> String {
    format!("scale_vaapi=w={w}:h={h}")
}

#[cfg(target_os = "linux")]
pub struct DrmFrame(frame::Video);

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
pub struct DrmPlane<'a> {
    pub fd: std::os::fd::BorrowedFd<'a>,
    pub offset: u64,
    pub pitch: u64,
    pub modifier: u64,
}

#[cfg(target_os = "linux")]
mod drm {
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

    pub fn planes(&self) -> Vec<DrmPlane<'_>> {
        // SAFETY: `data[0]` is the mapping's DRM descriptor; its files stay open as long as `self`.
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

    #[cfg(target_os = "linux")]
    #[test]
    fn a_kept_frame_waits_until_the_copy_settles() {
        assert!(super::handed_back(Err("sync"), 4u8).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_settled_copy_returns_the_kept_frame() {
        assert_eq!(super::handed_back(Ok::<(), &str>(()), 4u8).unwrap(), 4);
    }

    #[test]
    fn shrinking_scales_to_the_size() {
        assert_eq!(shrink_spec((960, 540)), "scale_vaapi=w=960:h=540");
    }
}
