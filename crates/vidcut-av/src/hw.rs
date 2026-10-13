//! The GPU's video decoders, whatever the GPU: FFmpeg's hardware devices.
//! NVIDIA's NVDEC on CUDA ([`crate::cuda`]); on other GPUs VA-API on Linux
//! (AMD, Intel) and Direct3D 11 on Windows (AMD, Intel). A reader decoding
//! on one gives its frames in that device's memory: CUDA's stay there for a
//! display that shows them from there, every other kind comes down to
//! memory as it is decoded, its surface going back to the decoder at once.
//!
//! `VIDCUT_HWDEC` picks one for tests and debugging: `nvdec`, `vaapi`
//! (`vaapi:/dev/dri/renderD129` for a render node), `d3d11va`
//! (`d3d11va:1` for an adapter by its index), or `none`.

use std::ptr;
use std::sync::{Arc, Mutex, OnceLock};

use crate::cuda::CudaDevice;
use crate::error::{Error, Result};
use crate::ffmpeg::{self, ffi, format, frame};

/// A kind of GPU decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// NVIDIA's, on CUDA.
    Nvdec,
    /// VA-API (Linux): Mesa's for AMD, Intel's own.
    Vaapi,
    /// Direct3D 11 video decoding (Windows).
    D3d11va,
}

impl Kind {
    /// A short name: `nvdec`, `vaapi` or `d3d11va`.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Nvdec => "nvdec",
            Kind::Vaapi => "vaapi",
            Kind::D3d11va => "d3d11va",
        }
    }

    /// The kind a [`name`](Self::name) names.
    pub fn named(name: &str) -> Option<Self> {
        [Kind::Nvdec, Kind::Vaapi, Kind::D3d11va]
            .into_iter()
            .find(|k| k.name() == name)
    }

    /// What it is called, for people: `NVDEC`, `VA-API`, `Direct3D 11`.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Nvdec => "NVDEC",
            Kind::Vaapi => "VA-API",
            Kind::D3d11va => "Direct3D 11",
        }
    }

    pub(crate) fn device_type(self) -> ffi::AVHWDeviceType {
        use ffi::AVHWDeviceType as T;
        match self {
            Kind::Nvdec => T::AV_HWDEVICE_TYPE_CUDA,
            Kind::Vaapi => T::AV_HWDEVICE_TYPE_VAAPI,
            Kind::D3d11va => T::AV_HWDEVICE_TYPE_D3D11VA,
        }
    }

    /// The format of its frames: where their pictures are.
    pub(crate) fn pixel(self) -> format::Pixel {
        match self {
            Kind::Nvdec => format::Pixel::CUDA,
            Kind::Vaapi => format::Pixel::VAAPI,
            Kind::D3d11va => format::Pixel::D3D11,
        }
    }
}

/// A reference to one of FFmpeg's hardware device contexts.
pub(crate) struct Device(pub(crate) *mut ffi::AVBufferRef);

// SAFETY: a reference to a hardware device context may be used, and
// dropped, on any thread.
unsafe impl Send for Device {}
unsafe impl Sync for Device {}

impl Clone for Device {
    fn clone(&self) -> Self {
        Self(unsafe { ffi::av_buffer_ref(self.0) })
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe { ffi::av_buffer_unref(&mut self.0) };
    }
}

impl Device {
    /// FFmpeg's device of `kind` on `on`: a render node, an adapter's
    /// index; the default one when empty.
    fn open(kind: Kind, on: &str) -> Result<Self> {
        Self::of_type(kind.device_type(), on)
    }

    /// QSV's device on `on`: a render node (Linux) or an adapter's index
    /// (Windows), the first Intel GPU when empty. FFmpeg reads QSV's own
    /// device string as the implementation to load (`1` is the software
    /// one), so the GPU goes in as the device QSV runs on top of
    /// (`child_device`): VA-API's on Linux, Direct3D 11's on Windows.
    pub(crate) fn qsv(on: &str) -> Result<Self> {
        crate::init();
        let mut options = ffmpeg::Dictionary::new();
        options.set(
            "child_device_type",
            if cfg!(windows) { "d3d11va" } else { "vaapi" },
        );
        if !on.is_empty() {
            options.set("child_device", on);
        }
        let mut device = ptr::null_mut();
        // SAFETY: the options' dictionary is FFmpeg's to read and is freed
        // after; FFmpeg makes the device from it.
        let ret = unsafe {
            let mut raw = options.disown();
            let ret = ffi::av_hwdevice_ctx_create(
                &mut device,
                ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_QSV,
                ptr::null(),
                raw,
                0,
            );
            ffi::av_dict_free(&mut raw);
            ret
        };
        if ret < 0 || device.is_null() {
            let on = if on.is_empty() {
                "the first Intel GPU"
            } else {
                on
            };
            return Err(Error::Ffmpeg {
                context: format!("opening QSV on {on}"),
                source: ffmpeg::Error::from(ret),
            });
        }
        Ok(Self(device))
    }

    /// FFmpeg's device of type `ty` on `on`, as [`open`](Self::open): for
    /// an encoder, which runs on one too.
    pub(crate) fn of_type(ty: ffi::AVHWDeviceType, on: &str) -> Result<Self> {
        crate::init();
        let name = std::ffi::CString::new(on)
            .map_err(|_| Error::Invalid(format!("a device name with a NUL: {on:?}")))?;
        let mut device = ptr::null_mut();
        // SAFETY: FFmpeg makes the device; the name is NUL-terminated.
        let ret = unsafe {
            ffi::av_hwdevice_ctx_create(
                &mut device,
                ty,
                if on.is_empty() {
                    ptr::null()
                } else {
                    name.as_ptr()
                },
                ptr::null_mut(),
                0,
            )
        };
        if ret < 0 || device.is_null() {
            let on = if on.is_empty() {
                "the default device"
            } else {
                on
            };
            // SAFETY: FFmpeg's static name of a type, or null.
            let what = unsafe {
                let n = ffi::av_hwdevice_get_type_name(ty);
                if n.is_null() {
                    "a GPU device".into()
                } else {
                    std::ffi::CStr::from_ptr(n).to_string_lossy()
                }
            };
            return Err(Error::Ffmpeg {
                context: format!("opening {what} on {on}"),
                source: ffmpeg::Error::from(ret),
            });
        }
        Ok(Self(device))
    }
}

/// A device the GPU decodes on.
#[derive(Clone)]
pub struct GpuDecoder {
    kind: Kind,
    device: Arc<Device>,
    /// What it is on: a render node, an adapter's index; empty for the
    /// default one.
    on: String,
    /// NVDEC's CUDA device, with what it does besides decoding.
    cuda: Option<CudaDevice>,
    /// Its GPU's PCI vendor, where known (Direct3D 11's adapter's).
    vendor: Option<u32>,
}

/// AMD's PCI vendor id.
const AMD: u32 = 0x1002;

/// The decoding profiles [`GpuDecoder::hardware_decodes`] asks a driver for.
#[derive(Clone, Copy)]
enum Profile {
    H264High,
    HevcMain,
    HevcMain10,
    Vp9Profile0,
    Vp9Profile2,
    Av1Main,
}

impl GpuDecoder {
    /// NVDEC, on `cuda`.
    pub fn nvdec(cuda: CudaDevice) -> Self {
        Self {
            kind: Kind::Nvdec,
            device: Arc::new(Device(unsafe { ffi::av_buffer_ref(cuda.as_ptr()) })),
            on: String::new(),
            cuda: Some(cuda),
            vendor: None,
        }
    }

    /// `kind` on `on` (a render node for VA-API, an adapter's index for
    /// Direct3D 11; the default one when empty). NVDEC is on the process's
    /// CUDA device.
    pub fn open(kind: Kind, on: &str) -> Result<Self> {
        if kind == Kind::Nvdec {
            return CudaDevice::open().map(Self::nvdec);
        }
        let device = Device::open(kind, on)?;
        #[cfg(windows)]
        // SAFETY: a D3D11VA device FFmpeg made.
        let vendor = (kind == Kind::D3d11va)
            .then(|| unsafe { windows::desc(&device) }.map(|d| d.VendorId))
            .flatten();
        #[cfg(not(windows))]
        let vendor = None;
        Ok(Self {
            kind,
            device: Arc::new(device),
            on: on.to_string(),
            cuda: None,
            vendor,
        })
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Whether it decodes `codec` right, as far as known. AMD's Direct3D 11
    /// decoder loses its way in H.264 sought into an open GOP (a keyframe
    /// that isn't IDR): some frames on, it shows one picture over and over,
    /// where NVIDIA's and VA-API on Intel decode on. The CPU decodes H.264
    /// there.
    pub fn decodes(&self, codec: ffmpeg::codec::Id) -> bool {
        !(self.kind == Kind::D3d11va
            && self.vendor == Some(AMD)
            && codec == ffmpeg::codec::Id::H264)
    }

    /// Whether its hardware decodes `codec` in `bits` (8 or 10), as its
    /// driver lists the profiles it decodes (VA-API's; Direct3D 11's video
    /// device's): H.264 High, HEVC Main or Main 10, VP9 profile 0 or 2, AV1
    /// Main. `None` where the driver isn't asked (NVDEC) or doesn't say.
    /// What [`decodes`](Self::decodes) knows of streams decoded wrong is
    /// apart from this.
    pub fn hardware_decodes(&self, codec: ffmpeg::codec::Id, bits: u8) -> Option<bool> {
        use ffmpeg::codec::Id;
        let profile = match (codec, bits) {
            (Id::H264, 8) => Profile::H264High,
            (Id::HEVC, 8) => Profile::HevcMain,
            (Id::HEVC, 10) => Profile::HevcMain10,
            (Id::VP9, 8) => Profile::Vp9Profile0,
            (Id::VP9, 10) => Profile::Vp9Profile2,
            (Id::AV1, 8 | 10) => Profile::Av1Main,
            _ => return None,
        };
        match self.kind {
            Kind::Nvdec => None,
            // SAFETY: a VA-API device FFmpeg made.
            #[cfg(target_os = "linux")]
            Kind::Vaapi => unsafe { va_lists(&self.device, profile) },
            // SAFETY: a D3D11VA device FFmpeg made.
            #[cfg(windows)]
            Kind::D3d11va => unsafe { windows::lists(&self.device, profile) },
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }

    /// The render node or adapter it is on; empty for the default one.
    pub fn on(&self) -> &str {
        &self.on
    }

    /// NVDEC's CUDA device.
    pub fn cuda(&self) -> Option<&CudaDevice> {
        self.cuda.as_ref()
    }

    /// The LUID of the adapter a Direct3D 11 decoder is on (Windows), which
    /// says which GPU it is: a device importing its textures must be on that
    /// one.
    pub fn luid(&self) -> Option<[u8; 8]> {
        #[cfg(windows)]
        if self.kind == Kind::D3d11va {
            // SAFETY: a D3D11VA device.
            let luid = unsafe { windows::desc(&self.device) }?.AdapterLuid;
            let mut bytes = [0; 8];
            bytes[..4].copy_from_slice(&luid.LowPart.to_le_bytes());
            bytes[4..].copy_from_slice(&luid.HighPart.to_le_bytes());
            return Some(bytes);
        }
        None
    }

    pub(crate) fn as_ptr(&self) -> *mut ffi::AVBufferRef {
        self.device.0
    }

    /// The best this computer has, opened once for the process: NVDEC on
    /// an NVIDIA GPU, else VA-API on the first render node that isn't
    /// NVIDIA's (Linux) or Direct3D 11 on the first adapter that isn't
    /// (Windows); `VIDCUT_HWDEC` says otherwise. `None` without any.
    pub fn best() -> Option<Self> {
        static BEST: OnceLock<Option<GpuDecoder>> = OnceLock::new();
        BEST.get_or_init(|| match std::env::var("VIDCUT_HWDEC") {
            Ok(asked) => Self::asked(&asked),
            Err(_) => CudaDevice::open()
                .ok()
                .map(Self::nvdec)
                .or_else(|| Self::others().into_iter().next()),
        })
        .clone()
    }

    /// Every decoder this computer has: NVDEC, and the other GPUs'.
    pub fn all() -> Vec<Self> {
        let mut all: Vec<Self> = CudaDevice::open()
            .ok()
            .map(Self::nvdec)
            .into_iter()
            .collect();
        all.extend(Self::others());
        all
    }

    /// The decoder `VIDCUT_HWDEC` names: `kind[:on]`, or `none`.
    fn asked(asked: &str) -> Option<Self> {
        let (kind, on) = asked.split_once(':').unwrap_or((asked, ""));
        Self::open(Kind::named(kind)?, on).ok()
    }

    /// The decoders of the GPUs other than NVIDIA's (NVDEC decodes there):
    /// VA-API on each render node (Linux), Direct3D 11 on each adapter
    /// (Windows).
    fn others() -> Vec<Self> {
        #[cfg(windows)]
        {
            windows::adapters()
                .into_iter()
                .filter(|&(_, vendor)| vendor != windows::NVIDIA)
                .filter_map(|(i, _)| Self::open(Kind::D3d11va, &i.to_string()).ok())
                .collect()
        }
        #[cfg(not(windows))]
        {
            crate::export::encode::render_nodes()
                .into_iter()
                .filter(|(_, driver)| driver != "nvidia")
                .filter_map(|(node, _)| Self::open(Kind::Vaapi, &node).ok())
                .collect()
        }
    }
}

#[cfg(windows)]
pub(crate) mod windows {
    use std::ffi::c_void;

    use ::windows::Win32::Graphics::Direct3D11::ID3D11Device;
    use ::windows::Win32::Graphics::Dxgi::{DXGI_ADAPTER_DESC, IDXGIDevice};
    use ::windows::core::Interface;

    use super::{Device, Kind, Profile};
    use crate::ffmpeg::ffi;

    /// PCI vendor ids.
    pub(crate) const NVIDIA: u32 = 0x10de;
    pub(crate) const AMD: u32 = super::AMD;
    pub(crate) const INTEL: u32 = 0x8086;

    /// Microsoft's: its Basic Render Driver, which draws on the CPU and
    /// decodes nothing.
    const MICROSOFT: u32 = 0x1414;

    /// The adapters with a GPU behind them: their index, what Direct3D 11
    /// is opened on, and their PCI vendor.
    pub(crate) fn adapters() -> Vec<(u32, u32)> {
        let mut adapters = Vec::new();
        for i in 0..8 {
            let Ok(device) = Device::open(Kind::D3d11va, &i.to_string()) else {
                break;
            };
            // SAFETY: a D3D11VA device FFmpeg made.
            match unsafe { desc(&device) }.map(|d| d.VendorId) {
                Some(MICROSOFT) | None => {}
                Some(v) => adapters.push((i, v)),
            }
        }
        adapters
    }

    /// Whether a D3D11VA device's video device lists `profile` among its
    /// decoders' (`GetVideoDecoderProfile`).
    ///
    /// # Safety
    /// `device` is a D3D11VA device context.
    pub(super) unsafe fn lists(device: &Device, profile: Profile) -> Option<bool> {
        use ::windows::Win32::Graphics::Direct3D11::{
            D3D11_DECODER_PROFILE_AV1_VLD_PROFILE0, D3D11_DECODER_PROFILE_H264_VLD_NOFGT,
            D3D11_DECODER_PROFILE_HEVC_VLD_MAIN, D3D11_DECODER_PROFILE_HEVC_VLD_MAIN10,
            D3D11_DECODER_PROFILE_VP9_VLD_10BIT_PROFILE2, D3D11_DECODER_PROFILE_VP9_VLD_PROFILE0,
            ID3D11VideoDevice,
        };
        let want = match profile {
            Profile::H264High => D3D11_DECODER_PROFILE_H264_VLD_NOFGT,
            Profile::HevcMain => D3D11_DECODER_PROFILE_HEVC_VLD_MAIN,
            Profile::HevcMain10 => D3D11_DECODER_PROFILE_HEVC_VLD_MAIN10,
            Profile::Vp9Profile0 => D3D11_DECODER_PROFILE_VP9_VLD_PROFILE0,
            Profile::Vp9Profile2 => D3D11_DECODER_PROFILE_VP9_VLD_10BIT_PROFILE2,
            Profile::Av1Main => D3D11_DECODER_PROFILE_AV1_VLD_PROFILE0,
        };
        // SAFETY: a D3D11VA device's context is an AVD3D11VADeviceContext,
        // whose video device FFmpeg asked its device for and which lives as
        // long as it.
        unsafe {
            let ctx = (*device.0).data as *const ffi::AVHWDeviceContext;
            let hw = (*ctx).hwctx as *const ffi::AVD3D11VADeviceContext;
            let raw = (*hw).video_device as *mut c_void;
            let video = ID3D11VideoDevice::from_raw_borrowed(&raw)?;
            Some(
                (0..video.GetVideoDecoderProfileCount())
                    .any(|i| video.GetVideoDecoderProfile(i).is_ok_and(|g| g == want)),
            )
        }
    }

    /// The description of the adapter a D3D11VA device is on: its Direct3D
    /// device asked for its DXGI device, that for its adapter.
    ///
    /// # Safety
    /// `device` is a D3D11VA device context.
    pub(super) unsafe fn desc(device: &Device) -> Option<DXGI_ADAPTER_DESC> {
        // SAFETY: a D3D11VA device's context is an AVD3D11VADeviceContext,
        // whose Direct3D device lives as long as it.
        unsafe {
            let ctx = (*device.0).data as *const ffi::AVHWDeviceContext;
            let hw = (*ctx).hwctx as *const ffi::AVD3D11VADeviceContext;
            let raw = (*hw).device as *mut c_void;
            let d3d11 = ID3D11Device::from_raw_borrowed(&raw)?;
            let dxgi: IDXGIDevice = d3d11.cast().ok()?;
            dxgi.GetAdapter().ok()?.GetDesc().ok()
        }
    }
}

/// Whether `frame` is in GPU memory: a hardware frame of any kind.
pub fn on_gpu(frame: &frame::Video) -> bool {
    // SAFETY: reads the frame's own field.
    unsafe { !(*frame.as_ptr()).hw_frames_ctx.is_null() }
}

/// The layout of a frame in GPU memory: its frames context's software
/// format (NV12, P010...); a frame in memory's own.
pub(crate) fn sw_format(frame: &frame::Video) -> format::Pixel {
    // SAFETY: a hardware frame holds a reference to its frames context,
    // whose data is an AVHWFramesContext.
    unsafe {
        let ctx = (*frame.as_ptr()).hw_frames_ctx;
        if ctx.is_null() {
            frame.format()
        } else {
            let frames = (*ctx).data as *const ffi::AVHWFramesContext;
            format::Pixel::from((*frames).sw_format)
        }
    }
}

/// A frame in GPU memory copied down to memory, in its frames' layout (NV12,
/// P010) at its own size, keeping its properties: colors, aspect,
/// timestamp.
pub fn download(src: &frame::Video) -> Result<frame::Video> {
    // SAFETY: a valid frame.
    let Some(display) = (unsafe { intel_va(src) }) else {
        return fetched(src);
    };
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

/// `src` copied down FFmpeg's way.
fn fetched(src: &frame::Video) -> Result<frame::Video> {
    let mut dst = frame::Video::empty();
    // SAFETY: both frames are valid; the destination is allocated by the
    // transfer, in the source frames' software format.
    unsafe {
        let ret = ffi::av_hwframe_transfer_data(dst.as_mut_ptr(), src.as_ptr(), 0);
        if ret < 0 {
            return Err(Error::Ffmpeg {
                context: "copying a frame down from the GPU".into(),
                source: ffmpeg::Error::from(ret),
            });
        }
    }
    props(&mut dst, src)?;
    Ok(dst)
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

/// `src`'s properties onto `dst`.
fn props(dst: &mut frame::Video, src: &frame::Video) -> Result<()> {
    // SAFETY: both frames are valid.
    let ret = unsafe { ffi::av_frame_copy_props(dst.as_mut_ptr(), src.as_ptr()) };
    if ret < 0 {
        return Err(Error::Ffmpeg {
            context: "copying a frame's properties".into(),
            source: ffmpeg::Error::from(ret),
        });
    }
    Ok(())
}

/// Whether a VA-API device's driver lists `profile` with the entry point
/// for decoding (`VAEntrypointVLD`); `None` without libva to ask.
///
/// # Safety
/// `device` is a VA-API device context.
#[cfg(target_os = "linux")]
unsafe fn va_lists(device: &Device, profile: Profile) -> Option<bool> {
    use std::ffi::{c_int, c_void};
    /// `AVVAAPIDeviceContext`.
    #[repr(C)]
    struct VaapiDevice {
        display: *mut c_void,
        driver_quirks: std::ffi::c_uint,
    }
    type Max = unsafe extern "C" fn(*mut c_void) -> c_int;
    type Entrypoints = unsafe extern "C" fn(*mut c_void, c_int, *mut c_int, *mut c_int) -> c_int;
    // va.h's `VAProfile` and `VAEntrypointVLD`.
    let va_profile = match profile {
        Profile::H264High => 7,
        Profile::HevcMain => 17,
        Profile::HevcMain10 => 18,
        Profile::Vp9Profile0 => 19,
        Profile::Vp9Profile2 => 21,
        Profile::Av1Main => 32,
    };
    const VLD: c_int = 1;
    // SAFETY: libva's functions as va.h declares them, looked up in the
    // process (loaded with FFmpeg); the device's context is an
    // AVVAAPIDeviceContext with its display; the list holds as many entry
    // points as libva says there can be.
    unsafe {
        let max = libc::dlsym(libc::RTLD_DEFAULT, c"vaMaxNumEntrypoints".as_ptr());
        let query = libc::dlsym(libc::RTLD_DEFAULT, c"vaQueryConfigEntrypoints".as_ptr());
        if max.is_null() || query.is_null() {
            return None;
        }
        let max = std::mem::transmute::<*mut c_void, Max>(max);
        let query = std::mem::transmute::<*mut c_void, Entrypoints>(query);
        let ctx = (*device.0).data as *const ffi::AVHWDeviceContext;
        let display = (*((*ctx).hwctx as *const VaapiDevice)).display;
        if display.is_null() {
            return None;
        }
        let mut entrypoints = vec![0; usize::try_from(max(display)).ok()?.max(1)];
        let mut n: c_int = 0;
        // A profile the driver lacks: an error, not an empty list.
        if query(display, va_profile, entrypoints.as_mut_ptr(), &mut n) != 0 {
            return Some(false);
        }
        let n = usize::try_from(n).unwrap_or(0).min(entrypoints.len());
        Some(entrypoints[..n].contains(&VLD))
    }
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
    static QUERY: OnceLock<Option<Query>> = OnceLock::new();
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

/// Copies frames kept on the GPU out of a decoder whose few surfaces don't
/// grow, so kept ones don't run it out: Direct3D 11's, into textures of
/// their own that another device can open (Windows). FFmpeg copies NVDEC's
/// itself, and VA-API's pool grows as it needs.
pub(crate) enum Keeper {
    #[cfg(windows)]
    D3d11(d3d11::Keeper),
}

/// What a keeper or shrinker is made for: the frames context a frame comes
/// from, its layout, and the frame's size.
type KeptLike = (usize, format::Pixel, u32, u32);

/// The frames context a hardware frame comes from, and what a keeper made
/// for it is made for ([`KeptLike`]).
fn frames_of(f: &frame::Video) -> Result<(*mut ffi::AVBufferRef, KeptLike)> {
    // SAFETY: reads the frame's own field, and a live buffer reference.
    unsafe {
        let frames = (*f.as_ptr()).hw_frames_ctx;
        if frames.is_null() {
            return Err(Error::Invalid("a GPU frame without its frames".into()));
        }
        let like = ((*frames).data as usize, sw_format(f), f.width(), f.height());
        Ok((frames, like))
    }
}

impl Keeper {
    /// `f` (a Direct3D 11 frame) copied, with its timestamp and colors;
    /// `keeper` made for frames like it first.
    pub(crate) fn copy(keeper: &mut Option<Keeper>, f: &frame::Video) -> Result<frame::Video> {
        #[cfg(windows)]
        if f.format() == format::Pixel::D3D11 {
            let (frames, like) = frames_of(f)?;
            if !matches!(keeper, Some(Keeper::D3d11(k)) if k.like == like) {
                *keeper = Some(Keeper::D3d11(d3d11::Keeper::new(f, frames, like)?));
            }
            let Some(Keeper::D3d11(k)) = keeper else {
                unreachable!("made above")
            };
            return k.copy(f);
        }
        let _ = keeper;
        Err(Error::Invalid(format!(
            "no keeping {:?} frames",
            f.format()
        )))
    }
}

/// The timestamp and colors of `from` onto `to`, a copy of its picture:
/// plain fields, for a frame of its own (`av_frame_copy_props` brings the
/// decoder's private reference along).
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

/// A filter graph of GPU frames like `f` (format `pixel`, from `frames`)
/// through `spec`.
fn gpu_graph(
    f: &frame::Video,
    frames: *mut ffi::AVBufferRef,
    pixel: ffi::AVPixelFormat,
    spec: &str,
) -> Result<ffmpeg::filter::Graph> {
    let mut graph = ffmpeg::filter::Graph::new();
    let tb = ffi::AVRational { num: 1, den: 1000 };
    crate::cuda::add_source(
        &mut graph,
        c"in",
        pixel,
        (f.width(), f.height()),
        (tb, f.aspect_ratio()),
        frames,
        None,
    )?;
    let sink = ffmpeg::filter::find("buffersink")
        .ok_or_else(|| Error::Invalid("no buffersink filter".into()))?;
    let failed = |context: String| move |source| Error::Ffmpeg { context, source };
    graph
        .add(&sink, "out", "")
        .map_err(failed("creating a GPU filter's sink".into()))?;
    graph
        .output("in", 0)
        .and_then(|p| p.input("out", 0))
        .and_then(|p| p.parse(spec))
        .map_err(failed(format!("building {spec}")))?;
    graph
        .validate()
        .map_err(failed(format!("configuring {spec}")))?;
    Ok(graph)
}

/// `f` through `graph`'s one filter: the frame it gives.
fn run_graph(
    graph: &mut ffmpeg::filter::Graph,
    f: &frame::Video,
    doing: &str,
) -> Result<frame::Video> {
    // The source takes the frame it is given: another reference.
    let mut given = frame::Video::empty();
    // SAFETY: both frames are valid.
    if unsafe { ffi::av_frame_ref(given.as_mut_ptr(), f.as_ptr()) } < 0 {
        return Err(Error::Invalid("no memory for a frame".into()));
    }
    let failed = |source| Error::Ffmpeg {
        context: doing.to_string(),
        source,
    };
    graph
        .get("in")
        .expect("source")
        .source()
        .add(&given)
        .map_err(failed)?;
    let mut out = frame::Video::empty();
    graph
        .get("out")
        .expect("sink")
        .sink()
        .frame(&mut out)
        .map_err(failed)?;
    Ok(out)
}

/// Scales frames down on the GPU (VA-API's, Direct3D 11's and CUDA's), so
/// they come down to memory small, in the frames' own layout (NV12, P010).
pub(crate) struct Shrinker {
    /// `None`: the GPU won't scale these frames, so they come down whole.
    scaling: Option<Scaling>,
    /// The frames context, layout and frame size it was made for, and the
    /// size it scales to.
    input: (KeptLike, (u32, u32)),
}

enum Scaling {
    /// `scale_vaapi`, or `scale_cuda` in steps.
    Filters(ffmpeg::filter::Graph),
    /// The GPU's video processor.
    #[cfg(windows)]
    D3d11(d3d11::Scaler),
}

impl Scaling {
    fn new(f: &frame::Video, frames: *mut ffi::AVBufferRef, (w, h): (u32, u32)) -> Result<Self> {
        match f.format() {
            format::Pixel::VAAPI => {
                let pixel = ffi::AVPixelFormat::AV_PIX_FMT_VAAPI;
                let spec = format!("scale_vaapi=w={w}:h={h}");
                Ok(Self::Filters(gpu_graph(f, frames, pixel, &spec)?))
            }
            format::Pixel::CUDA => {
                let pixel = ffi::AVPixelFormat::AV_PIX_FMT_CUDA;
                let spec = crate::cuda::scaling((f.width(), f.height()), (w, h), "").join(",");
                Ok(Self::Filters(gpu_graph(f, frames, pixel, &spec)?))
            }
            #[cfg(windows)]
            format::Pixel::D3D11 => Ok(Self::D3d11(d3d11::Scaler::new(f, frames, (w, h))?)),
            other => Err(Error::Invalid(format!("no scaling {other:?} frames"))),
        }
    }
}

impl Shrinker {
    /// `f` scaled to `size` (even) on the GPU and copied down, with its
    /// timestamp and colors; `shrinker` made for frames like it first. A
    /// GPU that won't scale them (its driver lacks the layout, or changes
    /// the levels) copies them down whole.
    pub(crate) fn down(
        shrinker: &mut Option<Shrinker>,
        f: &frame::Video,
        size: (u32, u32),
    ) -> Result<frame::Video> {
        let (frames, of) = frames_of(f)?;
        let input = (of, size);
        if shrinker.as_ref().is_none_or(|s| s.input != input) {
            let scaling = Scaling::new(f, frames, size).ok();
            *shrinker = Some(Self { scaling, input });
        }
        let shrinker = shrinker.as_mut().expect("made above");
        let scaled = match &mut shrinker.scaling {
            Some(Scaling::Filters(graph)) => run_graph(graph, f, "scaling a frame on the GPU"),
            #[cfg(windows)]
            Some(Scaling::D3d11(scaler)) => scaler.scale(f),
            None => return download(f),
        };
        match scaled.and_then(|scaled| download(&scaled)) {
            Ok(mut small) => {
                same_picture(&mut small, f);
                crate::color::keep_matrix(&mut small, f);
                Ok(small)
            }
            // The GPU failed it after all: these frames come down whole.
            Err(_) => {
                shrinker.scaling = None;
                download(f)
            }
        }
    }
}

/// Direct3D 11's frames on the GPU (Windows): kept, each copied out of the
/// decoder's texture array into a texture of its own, which another device
/// opens by an NT handle; and scaled by the GPU's video processor.
#[cfg(windows)]
pub(crate) mod d3d11 {
    use std::ffi::c_void;
    use std::mem::ManuallyDrop;

    use ::windows::Win32::Foundation::RECT;
    use ::windows::Win32::Graphics::Direct3D11::{
        D3D11_BIND_DECODER, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX,
        D3D11_QUERY_DESC, D3D11_QUERY_EVENT, D3D11_RESOURCE_MISC_SHARED,
        D3D11_RESOURCE_MISC_SHARED_NTHANDLE, D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV,
        D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
        D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT, D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT,
        D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
        D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
        D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D, ID3D11Device,
        ID3D11DeviceContext, ID3D11Query, ID3D11Texture2D, ID3D11VideoContext1, ID3D11VideoDevice,
        ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
    };
    use ::windows::Win32::Graphics::Dxgi::Common::{
        DXGI_COLOR_SPACE_TYPE, DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P601,
        DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P709, DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P2020,
        DXGI_COLOR_SPACE_YCBCR_FULL_GHLG_TOPLEFT_P2020,
        DXGI_COLOR_SPACE_YCBCR_STUDIO_G2084_LEFT_P2020, DXGI_FORMAT_NV12, DXGI_FORMAT_P010,
    };
    use ::windows::core::Interface;

    use super::*;

    /// `AVD3D11VADeviceContext`.
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

    /// `AVD3D11VAFramesContext`.
    #[repr(C)]
    struct FramesContext {
        texture: *mut c_void,
        bind_flags: u32,
        misc_flags: u32,
        texture_infos: *mut c_void,
    }

    /// The Direct3D 11 device of `frames`, a D3D11 frames context.
    ///
    /// # Safety
    /// `frames` is a live D3D11 frames context.
    pub(crate) unsafe fn device_of(frames: *mut ffi::AVBufferRef) -> *const DeviceContext {
        // SAFETY: a D3D11 frames context's device's context is an
        // AVD3D11VADeviceContext.
        unsafe {
            let device = (*((*frames).data as *const ffi::AVHWFramesContext)).device_ctx;
            (*device).hwctx as *const DeviceContext
        }
    }

    /// `work` on `dev`'s immediate context, under FFmpeg's lock for it, as
    /// its decoders use it.
    ///
    /// # Safety
    /// `dev` is a live device context.
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

    /// A D3D11 frame's texture and its index in it (a decoder's are
    /// arrays).
    ///
    /// # Safety
    /// `frame` is a live D3D11 frame.
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

    /// Frames on `source`'s device in its layout, at `size`: single
    /// textures bound and flagged so, in a pool that grows.
    ///
    /// # Safety
    /// `source` is a live D3D11 frames context.
    unsafe fn own_frames(
        source: *mut ffi::AVBufferRef,
        (w, h): (u32, u32),
        (bind_flags, misc_flags): (i32, i32),
        what: &str,
    ) -> Result<*mut ffi::AVBufferRef> {
        // SAFETY: frames made on the source frames' device, set up as
        // FFmpeg's D3D11 frames are before they start; freed on failure.
        unsafe {
            let src = (*source).data as *const ffi::AVHWFramesContext;
            let mut frames = ffi::av_hwframe_ctx_alloc((*src).device_ref);
            if frames.is_null() {
                return Err(Error::Invalid("no memory for frames".into()));
            }
            let fc = (*frames).data as *mut ffi::AVHWFramesContext;
            (*fc).format = ffi::AVPixelFormat::AV_PIX_FMT_D3D11;
            (*fc).sw_format = (*src).sw_format;
            (*fc).width = w as i32;
            (*fc).height = h as i32;
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

    /// A frame of `frames`' pool.
    ///
    /// # Safety
    /// `frames` is a live frames context.
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
        /// Its own frames: single textures, shareable, a pool that grows.
        frames: *mut ffi::AVBufferRef,
        /// The decoder's frames it copies out of, held so that another
        /// context can't come at their address while it is made for them.
        source: *mut ffi::AVBufferRef,
        /// Signalled once the copy before it is done.
        done: ID3D11Query,
        pub(super) like: super::KeptLike,
    }

    // SAFETY: the frames contexts are reference-counted and may be used on
    // any thread; the query is used under the device's lock.
    unsafe impl Send for Keeper {}

    impl Drop for Keeper {
        fn drop(&mut self) {
            // SAFETY: its own references.
            unsafe {
                ffi::av_buffer_unref(&mut self.frames);
                ffi::av_buffer_unref(&mut self.source);
            }
        }
    }

    /// How long a copy may take before the keeper gives up on it: far past
    /// any copy (a 2160p frame takes well under a millisecond), short of
    /// holding the decoder for long where the device was lost.
    const COPY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

    impl Keeper {
        pub(super) fn new(
            f: &frame::Video,
            source: *mut ffi::AVBufferRef,
            like: super::KeptLike,
        ) -> Result<Self> {
            // Textures of 4:2:0 frames have even sides.
            let size = (
                f.width().next_multiple_of(2),
                f.height().next_multiple_of(2),
            );
            let flags = (
                D3D11_BIND_SHADER_RESOURCE.0,
                D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0,
            );
            // SAFETY: the frame's own frames context; the frames made are
            // freed on failure.
            unsafe {
                let mut frames = own_frames(source, size, flags, "to keep")?;
                let device = ID3D11Device::from_raw_borrowed(&(*device_of(frames)).device);
                let query = device.and_then(|d| {
                    let desc = D3D11_QUERY_DESC {
                        Query: D3D11_QUERY_EVENT,
                        MiscFlags: 0,
                    };
                    let mut query = None;
                    d.CreateQuery(&desc, Some(&mut query)).ok().and(query)
                });
                let Some(done) = query else {
                    ffi::av_buffer_unref(&mut frames);
                    return Err(Error::Invalid("no Direct3D 11 query".into()));
                };
                let source = ffi::av_buffer_ref(source);
                if source.is_null() {
                    ffi::av_buffer_unref(&mut frames);
                    return Err(Error::Invalid("no memory for frames".into()));
                }
                Ok(Self {
                    frames,
                    source,
                    done,
                    like,
                })
            }
        }

        /// `f` copied into a texture of its own, the copy done when this
        /// returns: another device reads it next, which can't wait for
        /// Direct3D 11's work.
        pub(super) fn copy(&mut self, f: &frame::Video) -> Result<frame::Video> {
            // SAFETY: a frame from its own pool; the device's immediate
            // context used under its lock; the textures are the frames'
            // (array and index).
            unsafe {
                let mut out = pooled(self.frames)?;
                let dev = &*device_of(self.frames);
                let context = ID3D11DeviceContext::from_raw_borrowed(&dev.device_context)
                    .ok_or_else(|| Error::Invalid("no Direct3D 11 context".into()))?;
                let ((dst, di), (src, si)) = (texture(&out)?, texture(f)?);
                let region = D3D11_BOX {
                    left: 0,
                    top: 0,
                    front: 0,
                    right: f.width().next_multiple_of(2),
                    bottom: f.height().next_multiple_of(2),
                    back: 1,
                };
                locked(dev, || {
                    context.CopySubresourceRegion(&dst, di, 0, 0, 0, &src, si, Some(&region));
                    context.End(&self.done);
                    context.Flush();
                });
                // The event's BOOL turns true once the copy is done; until
                // then the call says not yet (`S_FALSE`). Failing (a device
                // lost) or past the deadline, the frame isn't kept.
                let started = std::time::Instant::now();
                loop {
                    let mut done = ::windows::core::BOOL(0);
                    let mut asked = Ok(());
                    locked(dev, || {
                        asked = context.GetData(
                            &self.done,
                            Some((&raw mut done).cast()),
                            size_of::<::windows::core::BOOL>() as u32,
                            0,
                        );
                    });
                    if let Err(e) = asked {
                        return Err(Error::Invalid(format!(
                            "waiting for a Direct3D 11 copy: {e}"
                        )));
                    }
                    if done.as_bool() {
                        break;
                    }
                    if started.elapsed() > COPY_DEADLINE {
                        return Err(Error::Invalid("a Direct3D 11 copy didn't finish".into()));
                    }
                    std::thread::sleep(std::time::Duration::from_micros(100));
                }
                same_picture(&mut out, f);
                // The texture is the frame's size, rounded up to even.
                (*out.as_mut_ptr()).width = f.width() as i32;
                (*out.as_mut_ptr()).height = f.height() as i32;
                Ok(out)
            }
        }
    }

    /// Scales Direct3D 11's frames on the GPU's video processor, into
    /// textures of its own in their layout and colors. FFmpeg's
    /// `scale_d3d11` makes its textures for an encoder as well, which AMD's
    /// driver refuses, and scales the padding of the decoder's textures in.
    pub(crate) struct Scaler {
        /// Its own frames: single textures it draws to, a pool that grows.
        frames: *mut ffi::AVBufferRef,
        video: ID3D11VideoDevice,
        context: ID3D11VideoContext1,
        enumerator: ID3D11VideoProcessorEnumerator,
        processor: ID3D11VideoProcessor,
        size: (u32, u32),
        /// The color space the processor is told the frames come in and go
        /// out in: their own, so they're only scaled.
        space: DXGI_COLOR_SPACE_TYPE,
    }

    impl Drop for Scaler {
        fn drop(&mut self) {
            // SAFETY: its own reference.
            unsafe { ffi::av_buffer_unref(&mut self.frames) };
        }
    }

    impl Scaler {
        /// Scales frames like `f` (of `source`) to `size` (even), keeping
        /// their levels and colors; an error where the video processor
        /// doesn't take their layout, or changes a test picture of grays and
        /// colors (a driver's own processing).
        pub(super) fn new(
            f: &frame::Video,
            source: *mut ffi::AVBufferRef,
            size: (u32, u32),
        ) -> Result<Self> {
            let input = (f.width(), f.height());
            let mut scaler = Self::made(source, input, size, color_space(f))?;
            if !scaler.keeps_levels(source, input)? {
                return Err(Error::Invalid(
                    "the video processor changes the levels".into(),
                ));
            }
            Ok(scaler)
        }

        /// A processor of frames of `source`'s layout at `input` to
        /// `size`, told they come in and go out in `space`.
        fn made(
            source: *mut ffi::AVBufferRef,
            input: (u32, u32),
            size: (u32, u32),
            space: DXGI_COLOR_SPACE_TYPE,
        ) -> Result<Self> {
            let failed = |what: &'static str| {
                move |e: ::windows::core::Error| Error::Invalid(format!("{what}: {e}"))
            };
            // SAFETY: the device of a live frames context; the frames made
            // are freed with the scaler.
            unsafe {
                let layout = match format::Pixel::from(
                    (*((*source).data as *const ffi::AVHWFramesContext)).sw_format,
                ) {
                    format::Pixel::NV12 => DXGI_FORMAT_NV12,
                    format::Pixel::P010LE => DXGI_FORMAT_P010,
                    other => {
                        return Err(Error::Invalid(format!(
                            "no scaling {other:?} on Direct3D 11"
                        )));
                    }
                };
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
                let both = (D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT.0
                    | D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT.0)
                    as u32;
                let takes = enumerator
                    .CheckVideoProcessorFormat(layout)
                    .map_err(failed("no video processor"))?;
                if takes & both != both {
                    return Err(Error::Invalid(format!(
                        "the video processor doesn't scale {layout:?}"
                    )));
                }
                let processor = video
                    .CreateVideoProcessor(&enumerator, 0)
                    .map_err(failed("no video processor"))?;
                let flags = (D3D11_BIND_RENDER_TARGET.0, 0);
                let frames = own_frames(source, size, flags, "to scale to")?;
                Ok(Self {
                    frames,
                    video,
                    context,
                    enumerator,
                    processor,
                    size,
                    space,
                })
            }
        }

        /// Whether a test picture of grays and colors at `input`, in the
        /// layout of `source`'s frames, comes out of it at the levels it
        /// went in at.
        fn keeps_levels(
            &mut self,
            source: *mut ffi::AVBufferRef,
            input: (u32, u32),
        ) -> Result<bool> {
            // Each band's middle, a few samples across in luma and chroma,
            // must be there to read: smaller, the scaler isn't trusted and
            // the frames come down whole.
            let (ow, oh) = (self.size.0 as usize, self.size.1 as usize);
            if ow < 4 * BANDS.len() || oh < 8 {
                return Ok(false);
            }
            // SAFETY: a live frames context.
            let pixel = unsafe {
                format::Pixel::from((*((*source).data as *const ffi::AVHWFramesContext)).sw_format)
            };
            let wide = pixel == format::Pixel::P010LE;
            // 10-bit levels in a 16-bit word's top bits.
            let shift = if wide { 8 } else { 0 };
            // Textures of 4:2:0 frames have even sides.
            let size = (input.0.next_multiple_of(2), input.1.next_multiple_of(2));
            let (w, h) = (size.0 as usize, size.1 as usize);
            let bands = BANDS.len();
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
                    put(0, x, y, BANDS[x * bands / w][0]);
                }
            }
            for y in 0..h / 2 {
                for x in 0..w / 2 {
                    let [_, u, v] = BANDS[2 * x * bands / w];
                    put(1, 2 * x, y, u);
                    put(1, 2 * x + 1, y, v);
                }
            }
            // SAFETY: frames made for the test, their reference let go once
            // the frame holds its own; a fresh frame uploaded into.
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
            let got = download(&self.scale(&uploaded)?)?;
            let read = |plane: usize, x: usize, y: usize| {
                let (stride, data) = (got.stride(plane), got.data(plane));
                if wide {
                    u16::from_le_bytes([data[y * stride + 2 * x], data[y * stride + 2 * x + 1]])
                        >> 6
                } else {
                    u16::from(data[y * stride + x])
                }
            };
            // A level either way (in 8 bits), in the middle of each band.
            let (scale, near) = if wide { (4, 4) } else { (1, 1) };
            // Below black and above white only where nothing clips them.
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

        /// `f` scaled into a texture of its own. The work is queued on the
        /// device's immediate context, before what reads it next there (the
        /// copy down).
        pub(super) fn scale(&mut self, f: &frame::Video) -> Result<frame::Video> {
            let (w, h) = self.size;
            let whole = |w: u32, h: u32| RECT {
                left: 0,
                top: 0,
                right: w as i32,
                bottom: h as i32,
            };
            let failed = |what: &'static str| {
                move |e: ::windows::core::Error| Error::Invalid(format!("{what}: {e}"))
            };
            // SAFETY: a frame from its own pool; the immediate context used
            // under its lock; the views are of the frames' textures (array
            // and index), the input's released after the blit.
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
                        .CreateVideoProcessorInputView(
                            &src,
                            &self.enumerator,
                            &desc,
                            Some(&mut input),
                        )
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
                    // Their own colors in and out: scaled, not converted.
                    c.VideoProcessorSetStreamColorSpace1(p, 0, self.space);
                    c.VideoProcessorSetOutputColorSpace1(p, self.space);
                    // None of the driver's own enhancing.
                    c.VideoProcessorSetStreamAutoProcessingMode(p, 0, false);
                    // The picture alone: the decoder's textures are padded.
                    c.VideoProcessorSetStreamSourceRect(
                        p,
                        0,
                        true,
                        Some(&whole(f.width(), f.height())),
                    );
                    c.VideoProcessorSetStreamDestRect(p, 0, true, Some(&whole(w, h)));
                    c.VideoProcessorSetOutputTargetRect(p, true, Some(&whole(w, h)));
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

    /// The test picture's bands (8-bit levels, Y, Cb, Cr): grays from black
    /// to white, BT.709's red, green and blue, and levels below black and
    /// above white with chroma near its ends, which video keeps and a
    /// scaler mustn't clip.
    const BANDS: [[u16; 3]; 10] = [
        [16, 128, 128],
        [64, 128, 128],
        [128, 128, 128],
        [192, 128, 128],
        [235, 128, 128],
        [63, 102, 240],
        [173, 42, 26],
        [32, 240, 118],
        [4, 8, 128],
        [250, 128, 248],
    ];

    /// The last bands, outside video range.
    const OUTSIDE: usize = 2;

    /// The color space the video processor is told `f`'s values are in, and
    /// go out in: its matrix (untagged, as its size implies), in full range
    /// where DXGI has one, as nothing is converted. Told studio range, AMD's
    /// processor clips levels below black and above white (gotchas.md); PQ
    /// has studio range alone.
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

    /// Whether `space` is one of the full-range ones [`color_space`] tells,
    /// which keep every level.
    fn full_range(space: DXGI_COLOR_SPACE_TYPE) -> bool {
        space != DXGI_COLOR_SPACE_YCBCR_STUDIO_G2084_LEFT_P2020
    }
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
