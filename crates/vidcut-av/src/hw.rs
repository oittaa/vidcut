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
            .then(|| unsafe { windows::vendor(&device) })
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
    use std::ptr;

    use windows_sys::core::GUID;

    use super::{Device, Kind, Profile};
    use crate::ffmpeg::ffi;

    /// PCI vendor ids.
    pub(crate) const NVIDIA: u32 = 0x10de;
    pub(crate) const AMD: u32 = 0x1002;
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
            match unsafe { vendor(&device) } {
                Some(MICROSOFT) | None => {}
                Some(v) => adapters.push((i, v)),
            }
        }
        adapters
    }

    /// `DXGI_ADAPTER_DESC`.
    #[repr(C)]
    struct AdapterDesc {
        description: [u16; 128],
        vendor_id: u32,
        device_id: u32,
        sub_sys_id: u32,
        revision: u32,
        dedicated_video_memory: usize,
        dedicated_system_memory: usize,
        shared_system_memory: usize,
        adapter_luid: (u32, i32),
    }

    /// `IDXGIDevice`'s interface id.
    const IID_IDXGI_DEVICE: GUID = GUID::from_u128(0x54ec77fa_1377_44e6_8c32_88fd5f44c84c);

    /// Whether a D3D11VA device's video device lists `profile` among its
    /// decoders' (`GetVideoDecoderProfile`).
    ///
    /// # Safety
    /// `device` is a D3D11VA device context.
    pub(super) unsafe fn lists(device: &Device, profile: Profile) -> Option<bool> {
        // `D3D11_DECODER_PROFILE_*`.
        let want = GUID::from_u128(match profile {
            Profile::H264High => 0x1b81be68_a0c7_11d3_b984_00c04f2e73c5,
            Profile::HevcMain => 0x5b11d51b_2f4c_4452_bcc3_09f2a1160cc0,
            Profile::HevcMain10 => 0x107af0e0_ef1a_4d19_aba8_67a163073d13,
            Profile::Vp9Profile0 => 0x463707f8_a1d0_4585_876d_83aa6d60b89e,
            Profile::Vp9Profile2 => 0xa4c749ef_6ecf_48aa_8448_50a7a1165ff7,
            Profile::Av1Main => 0xb8be4ccb_cf53_46ba_8d59_d6b8a6da5d2a,
        });
        type Count = unsafe extern "system" fn(*mut c_void) -> u32;
        type Get = unsafe extern "system" fn(*mut c_void, u32, *mut GUID) -> i32;
        // SAFETY: `ID3D11VideoDevice`'s vtable after IUnknown's three: eight
        // `Create…` methods, then GetVideoDecoderProfileCount (11th) and
        // GetVideoDecoderProfile (12th). FFmpeg asks the device for it.
        unsafe {
            let ctx = (*device.0).data as *const ffi::AVHWDeviceContext;
            let hw = (*ctx).hwctx as *const ffi::AVD3D11VADeviceContext;
            let video = (*hw).video_device as *mut c_void;
            if video.is_null() {
                return None;
            }
            let count: Count = std::mem::transmute(method(video, 11));
            let get: Get = std::mem::transmute(method(video, 12));
            Some((0..count(video)).any(|i| {
                let mut g: GUID = std::mem::zeroed();
                get(video, i, &mut g) >= 0
                    && (g.data1, g.data2, g.data3, g.data4)
                        == (want.data1, want.data2, want.data3, want.data4)
            }))
        }
    }

    /// The `n`th method of a COM object's vtable.
    ///
    /// # Safety
    /// `object` is a live COM object with at least `n + 1` methods.
    unsafe fn method(object: *mut c_void, n: usize) -> usize {
        unsafe { *(*(object as *const *const usize)).add(n) }
    }

    /// The PCI vendor of the adapter a D3D11VA device is on: its Direct3D
    /// device asked for its DXGI device, that for its adapter, that for
    /// its description.
    ///
    /// # Safety
    /// `device` is a D3D11VA device context.
    pub(super) unsafe fn vendor(device: &Device) -> Option<u32> {
        type QueryInterface =
            unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32;
        type Release = unsafe extern "system" fn(*mut c_void) -> u32;
        type GetAdapter = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32;
        type GetDesc = unsafe extern "system" fn(*mut c_void, *mut AdapterDesc) -> i32;
        // SAFETY: the vtables' layouts are the interfaces' as declared:
        // IUnknown (QueryInterface, AddRef, Release); IDXGIDevice after
        // IDXGIObject's four (GetAdapter 7th); IDXGIAdapter after them and
        // EnumOutputs (GetDesc 8th). Every reference got is released.
        unsafe {
            let ctx = (*device.0).data as *const ffi::AVHWDeviceContext;
            let hw = (*ctx).hwctx as *const ffi::AVD3D11VADeviceContext;
            let d3d11 = (*hw).device as *mut c_void;
            if d3d11.is_null() {
                return None;
            }
            let query: QueryInterface = std::mem::transmute(method(d3d11, 0));
            let mut dxgi = ptr::null_mut();
            if query(d3d11, &IID_IDXGI_DEVICE, &mut dxgi) < 0 || dxgi.is_null() {
                return None;
            }
            let get_adapter: GetAdapter = std::mem::transmute(method(dxgi, 7));
            let release: Release = std::mem::transmute(method(dxgi, 2));
            let mut adapter = ptr::null_mut();
            let got = get_adapter(dxgi, &mut adapter);
            release(dxgi);
            if got < 0 || adapter.is_null() {
                return None;
            }
            let get_desc: GetDesc = std::mem::transmute(method(adapter, 8));
            let release: Release = std::mem::transmute(method(adapter, 2));
            let mut desc: AdapterDesc = std::mem::zeroed();
            let ok = get_desc(adapter, &mut desc) >= 0;
            release(adapter);
            ok.then_some(desc.vendor_id)
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
