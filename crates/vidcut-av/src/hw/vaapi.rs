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

/// The filter that shrinks VA-API's frames to `w` by `h`.
pub(super) fn shrink_spec((w, h): (u32, u32)) -> String {
    format!("scale_vaapi=w={w}:h={h}")
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
