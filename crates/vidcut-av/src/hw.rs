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
use std::sync::{Arc, OnceLock};

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

    use super::{Device, Kind};
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
pub(crate) fn on_gpu(frame: &frame::Video) -> bool {
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
    if unsafe { intel_va(src) }
        && let Some(f) = mapped_down(src)?
    {
        return Ok(f);
    }
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

/// `src` copied out of its surface mapped for reading itself
/// (`vaDeriveImage`), where FFmpeg's copy-down reads a copy the driver makes
/// of it (`vaGetImage`): on an Intel N150 a 2160p P010 frame took 15 ms of a
/// core so, 38 ms FFmpeg's way. `None` when it doesn't map.
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

/// `frame` is a VA-API frame of Intel's driver (iHD), by the vendor string
/// libva gives its display.
///
/// # Safety
/// `frame` is a valid frame.
#[cfg(target_os = "linux")]
unsafe fn intel_va(frame: &frame::Video) -> bool {
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
    let Some(query) = query else {
        return false;
    };
    // SAFETY: a hardware frame's frames context holds its device's, and a
    // VA-API device's context is an AVVAAPIDeviceContext with its display.
    unsafe {
        let frames = (*frame.as_ptr()).hw_frames_ctx;
        if frames.is_null() {
            return false;
        }
        let device = (*((*frames).data as *const ffi::AVHWFramesContext)).device_ctx;
        if device.is_null() || (*device).type_ != ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI {
            return false;
        }
        let va = (*device).hwctx as *const VaapiDevice;
        if va.is_null() || (*va).display.is_null() {
            return false;
        }
        let vendor = query((*va).display);
        !vendor.is_null() && CStr::from_ptr(vendor).to_bytes().starts_with(b"Intel iHD")
    }
}

/// VA-API is Linux's.
///
/// # Safety
/// None.
#[cfg(not(target_os = "linux"))]
unsafe fn intel_va(_: &frame::Video) -> bool {
    false
}

/// Copies frames kept on the GPU out of the decoder's few surfaces into a
/// pool of their own that grows, so kept ones don't run the decoder out
/// (FFmpeg does that with NVDEC's): VA-API's by `scale_vaapi` at their own
/// size, a copy on the GPU that keeps every byte; Direct3D 11's into
/// textures of their own that another device can open (Windows).
pub(crate) enum Keeper {
    Vaapi(VaapiKeeper),
    #[cfg(windows)]
    D3d11(d3d11::Keeper),
}

/// The frames context a hardware frame comes from, and what a keeper made
/// for it is made for: that context and the frame's size.
fn frames_of(f: &frame::Video) -> Result<(*mut ffi::AVBufferRef, (usize, u32, u32))> {
    // SAFETY: reads the frame's own field, and a live buffer reference.
    unsafe {
        let frames = (*f.as_ptr()).hw_frames_ctx;
        if frames.is_null() {
            return Err(Error::Invalid("a GPU frame without its frames".into()));
        }
        Ok((frames, ((*frames).data as usize, f.width(), f.height())))
    }
}

impl Keeper {
    /// `f` (a VA-API or Direct3D 11 frame) copied, with its timestamp and
    /// colors; `keeper` made for frames like it first.
    pub(crate) fn copy(keeper: &mut Option<Keeper>, f: &frame::Video) -> Result<frame::Video> {
        let (frames, input) = frames_of(f)?;
        match f.format() {
            format::Pixel::VAAPI => {
                if !matches!(keeper, Some(Keeper::Vaapi(k)) if k.input == input) {
                    *keeper = Some(Keeper::Vaapi(VaapiKeeper::new(f, frames, input)?));
                }
                let Some(Keeper::Vaapi(k)) = keeper else {
                    unreachable!("made above")
                };
                k.copy(f)
            }
            #[cfg(windows)]
            format::Pixel::D3D11 => {
                if !matches!(keeper, Some(Keeper::D3d11(k)) if k.input == input) {
                    *keeper = Some(Keeper::D3d11(d3d11::Keeper::new(f, frames, input)?));
                }
                let Some(Keeper::D3d11(k)) = keeper else {
                    unreachable!("made above")
                };
                k.copy(f)
            }
            other => Err(Error::Invalid(format!("no keeping {other:?} frames"))),
        }
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

/// VA-API's keeper: `scale_vaapi` at the frames' own size, whose output
/// pool grows.
pub(crate) struct VaapiKeeper {
    graph: ffmpeg::filter::Graph,
    /// The frames context and size it was made for.
    input: (usize, u32, u32),
}

impl VaapiKeeper {
    fn copy(&mut self, f: &frame::Video) -> Result<frame::Video> {
        // The source takes the frame it is given: another reference.
        let mut given = frame::Video::empty();
        // SAFETY: both frames are valid.
        if unsafe { ffi::av_frame_ref(given.as_mut_ptr(), f.as_ptr()) } < 0 {
            return Err(Error::Invalid("no memory for a frame".into()));
        }
        let failed = |source| Error::Ffmpeg {
            context: "copying a VA-API frame".into(),
            source,
        };
        self.graph
            .get("in")
            .expect("source")
            .source()
            .add(&given)
            .map_err(failed)?;
        let mut out = frame::Video::empty();
        self.graph
            .get("out")
            .expect("sink")
            .sink()
            .frame(&mut out)
            .map_err(failed)?;
        same_picture(&mut out, f);
        Ok(out)
    }

    fn new(
        f: &frame::Video,
        frames: *mut ffi::AVBufferRef,
        input: (usize, u32, u32),
    ) -> Result<Self> {
        let mut graph = ffmpeg::filter::Graph::new();
        let tb = ffi::AVRational { num: 1, den: 1000 };
        crate::cuda::add_source(
            &mut graph,
            c"in",
            ffi::AVPixelFormat::AV_PIX_FMT_VAAPI,
            (f.width(), f.height()),
            (tb, f.aspect_ratio()),
            frames,
            None,
        )?;
        let sink = ffmpeg::filter::find("buffersink")
            .ok_or_else(|| Error::Invalid("no buffersink filter".into()))?;
        let failed = |context: &str| {
            let context = context.to_string();
            move |source| Error::Ffmpeg { context, source }
        };
        graph
            .add(&sink, "out", "")
            .map_err(failed("creating the copy's sink"))?;
        graph
            .output("in", 0)
            .and_then(|p| p.input("out", 0))
            .and_then(|p| p.parse("scale_vaapi=mode=fast"))
            .map_err(failed("building scale_vaapi"))?;
        graph
            .validate()
            .map_err(failed("configuring scale_vaapi"))?;
        Ok(Self { graph, input })
    }
}

/// Direct3D 11's frames kept on the GPU (Windows): each copied out of the
/// decoder's texture array into a texture of its own, which another device
/// opens by an NT handle.
#[cfg(windows)]
pub(crate) mod d3d11 {
    use std::ffi::c_void;

    use ::windows::Win32::Graphics::Direct3D11::{
        D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_QUERY_DESC, D3D11_QUERY_EVENT,
        D3D11_RESOURCE_MISC_SHARED, D3D11_RESOURCE_MISC_SHARED_NTHANDLE, ID3D11Device,
        ID3D11DeviceContext, ID3D11Query, ID3D11Texture2D,
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

    pub(crate) struct Keeper {
        /// Its own frames: single textures, shareable, a pool that grows.
        frames: *mut ffi::AVBufferRef,
        /// Signalled once the copy before it is done.
        done: ID3D11Query,
        pub(super) input: (usize, u32, u32),
    }

    // SAFETY: the frames context is reference-counted and may be used on
    // any thread; the query is used under the device's lock.
    unsafe impl Send for Keeper {}

    impl Drop for Keeper {
        fn drop(&mut self) {
            // SAFETY: its own reference.
            unsafe { ffi::av_buffer_unref(&mut self.frames) };
        }
    }

    impl Keeper {
        pub(super) fn new(
            f: &frame::Video,
            source: *mut ffi::AVBufferRef,
            input: (usize, u32, u32),
        ) -> Result<Self> {
            // Textures of 4:2:0 frames have even sides.
            let (w, h) = (
                f.width().next_multiple_of(2),
                f.height().next_multiple_of(2),
            );
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
                (*hw).bind_flags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
                (*hw).misc_flags =
                    (D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0) as u32;
                let ret = ffi::av_hwframe_ctx_init(frames);
                if ret < 0 {
                    ffi::av_buffer_unref(&mut frames);
                    return Err(Error::Ffmpeg {
                        context: "making Direct3D 11 frames to keep".into(),
                        source: ffmpeg::Error::from(ret),
                    });
                }
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
                Ok(Self {
                    frames,
                    done,
                    input,
                })
            }
        }

        /// `f` copied into a texture of its own, the copy done when this
        /// returns: another device reads it next, which can't wait for
        /// Direct3D 11's work.
        pub(super) fn copy(&mut self, f: &frame::Video) -> Result<frame::Video> {
            let mut out = frame::Video::empty();
            // SAFETY: a frame from its own pool; the device's immediate
            // context used under FFmpeg's lock for it, as its decoders use
            // it; the textures are the frames' (array and index).
            unsafe {
                let ret = ffi::av_hwframe_get_buffer(self.frames, out.as_mut_ptr(), 0);
                if ret < 0 {
                    return Err(Error::Ffmpeg {
                        context: "getting a Direct3D 11 texture".into(),
                        source: ffmpeg::Error::from(ret),
                    });
                }
                let dev = &*device_of(self.frames);
                let context = ID3D11DeviceContext::from_raw_borrowed(&dev.device_context)
                    .ok_or_else(|| Error::Invalid("no Direct3D 11 context".into()))?;
                let texture = |frame: &frame::Video| {
                    let p = (*frame.as_ptr()).data[0] as *mut c_void;
                    let index = (*frame.as_ptr()).data[1] as usize as u32;
                    ID3D11Texture2D::from_raw_borrowed(&p).map(|t| (t.clone(), index))
                };
                let (Some((dst, di)), Some((src, si))) = (texture(&out), texture(f)) else {
                    return Err(Error::Invalid(
                        "a Direct3D 11 frame without a texture".into(),
                    ));
                };
                let region = D3D11_BOX {
                    left: 0,
                    top: 0,
                    front: 0,
                    right: f.width().next_multiple_of(2),
                    bottom: f.height().next_multiple_of(2),
                    back: 1,
                };
                let locked = |work: &mut dyn FnMut()| {
                    if let Some(lock) = dev.lock {
                        lock(dev.lock_ctx);
                    }
                    work();
                    if let Some(unlock) = dev.unlock {
                        unlock(dev.lock_ctx);
                    }
                };
                locked(&mut || {
                    context.CopySubresourceRegion(&dst, di, 0, 0, 0, &src, si, Some(&region));
                    context.End(&self.done);
                    context.Flush();
                });
                // The event's BOOL turns true once the copy is done (the call
                // succeeds either way).
                loop {
                    let mut done = ::windows::core::BOOL(0);
                    locked(&mut || {
                        let _ = context.GetData(
                            &self.done,
                            Some((&raw mut done).cast()),
                            size_of::<::windows::core::BOOL>() as u32,
                            0,
                        );
                    });
                    if done.as_bool() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_micros(100));
                }
                same_picture(&mut out, f);
                // The texture is the frame's size, rounded up to even.
                (*out.as_mut_ptr()).width = f.width() as i32;
                (*out.as_mut_ptr()).height = f.height() as i32;
            }
            Ok(out)
        }
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
