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
        crate::init();
        let name = std::ffi::CString::new(on)
            .map_err(|_| Error::Invalid(format!("a device name with a NUL: {on:?}")))?;
        let mut device = ptr::null_mut();
        // SAFETY: FFmpeg makes the device; the name is NUL-terminated.
        let ret = unsafe {
            ffi::av_hwdevice_ctx_create(
                &mut device,
                kind.device_type(),
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
            return Err(Error::Ffmpeg {
                context: format!("opening {} on {on}", kind.name()),
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
}

impl GpuDecoder {
    /// NVDEC, on `cuda`.
    pub fn nvdec(cuda: CudaDevice) -> Self {
        Self {
            kind: Kind::Nvdec,
            device: Arc::new(Device(unsafe { ffi::av_buffer_ref(cuda.as_ptr()) })),
            on: String::new(),
            cuda: Some(cuda),
        }
    }

    /// `kind` on `on` (a render node for VA-API, an adapter's index for
    /// Direct3D 11; the default one when empty). NVDEC is on the process's
    /// CUDA device.
    pub fn open(kind: Kind, on: &str) -> Result<Self> {
        if kind == Kind::Nvdec {
            return CudaDevice::open().map(Self::nvdec);
        }
        Ok(Self {
            kind,
            device: Arc::new(Device::open(kind, on)?),
            on: on.to_string(),
            cuda: None,
        })
    }

    pub fn kind(&self) -> Kind {
        self.kind
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
        let kind = match kind {
            "nvdec" => Kind::Nvdec,
            "vaapi" => Kind::Vaapi,
            "d3d11va" => Kind::D3d11va,
            _ => return None,
        };
        Self::open(kind, on).ok()
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
mod windows {
    use std::ffi::c_void;
    use std::ptr;

    use windows_sys::core::GUID;

    use super::{Device, Kind};
    use crate::ffmpeg::ffi;

    /// NVIDIA's PCI vendor id.
    pub(super) const NVIDIA: u32 = 0x10de;

    /// Microsoft's: its Basic Render Driver, which draws on the CPU and
    /// decodes nothing.
    const MICROSOFT: u32 = 0x1414;

    /// The adapters with a GPU behind them: their index, what Direct3D 11
    /// is opened on, and their PCI vendor.
    pub(super) fn adapters() -> Vec<(u32, u32)> {
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
    unsafe fn vendor(device: &Device) -> Option<u32> {
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
pub(crate) fn download(src: &frame::Video) -> Result<frame::Video> {
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
        let ret = ffi::av_frame_copy_props(dst.as_mut_ptr(), src.as_ptr());
        if ret < 0 {
            return Err(Error::Ffmpeg {
                context: "copying a frame's properties".into(),
                source: ffmpeg::Error::from(ret),
            });
        }
    }
    Ok(dst)
}
