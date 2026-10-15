//! The GPU's video decoders, whatever the GPU: FFmpeg's hardware devices.
//! NVIDIA's NVDEC on CUDA ([`crate::cuda`]), and ProRes, which NVDEC
//! doesn't decode, on Vulkan on the same GPU; on other GPUs VA-API on Linux
//! (AMD, Intel) and Direct3D 11 on Windows (AMD, Intel); VideoToolbox on a
//! Mac. A reader decoding on one gives its frames in that device's memory.
//!
//! `VIDCUT_HWDEC` picks one for tests and debugging: `nvdec` (`nvdec:1`
//! for a CUDA device by its index), `vaapi` (`vaapi:/dev/dri/renderD129`
//! for a render node), `d3d11va` (`d3d11va:1` for an adapter by its
//! index), `videotoolbox`, or `none`. Vulkan comes with `nvdec`.

#[cfg(windows)]
pub(crate) mod d3d11;
pub mod vaapi;
pub(crate) mod vt;
pub mod vulkan;

use std::ptr;
use std::sync::{Arc, Mutex};

use crate::cuda::CudaDevice;
use crate::error::{Error, Result};
use crate::ffmpeg::{self, ffi, format, frame};
use crate::route::{Ops, Place};

/// A kind of GPU decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// NVIDIA's, on CUDA.
    Nvdec,
    /// VA-API (Linux): Mesa's for AMD, Intel's own.
    Vaapi,
    /// Direct3D 11 video decoding (Windows).
    D3d11va,
    /// VideoToolbox (macOS).
    VideoToolbox,
    /// FFmpeg's Vulkan decoders on NVIDIA's GPU, for what NVDEC doesn't
    /// decode: ProRes.
    Vulkan,
}

impl Kind {
    /// A short name: `nvdec`, `vaapi`, `d3d11va`, `videotoolbox` or
    /// `vulkan`.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Nvdec => "nvdec",
            Kind::Vaapi => "vaapi",
            Kind::D3d11va => "d3d11va",
            Kind::VideoToolbox => "videotoolbox",
            Kind::Vulkan => "vulkan",
        }
    }

    const ALL: [Kind; 5] = [
        Kind::Nvdec,
        Kind::Vaapi,
        Kind::D3d11va,
        Kind::VideoToolbox,
        Kind::Vulkan,
    ];

    /// The kind a [`name`](Self::name) names.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    /// The kind a [`label`](Self::label) names.
    pub fn with_label(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.label() == label)
    }

    /// The kind whose GPU memory `f` is in; `None` for a frame in memory.
    pub fn of_frame(f: &frame::Video) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.pixel() == f.format())
    }

    /// The kind of FFmpeg's hardware device `device` (a codec's
    /// `hw_device_ctx`), when vidcut decodes on that kind.
    ///
    /// # Safety
    /// `device` is null or a reference to a hardware device context.
    pub(crate) unsafe fn of_device(device: *const ffi::AVBufferRef) -> Option<Self> {
        if device.is_null() {
            return None;
        }
        // SAFETY: a hardware device context's data, as the caller says.
        let ty = unsafe { (*((*device).data as *const ffi::AVHWDeviceContext)).type_ };
        Self::ALL.into_iter().find(|k| k.device_type() == ty)
    }

    /// What it is called, for people: `NVDEC`, `VA-API`, `Direct3D 11`,
    /// `VideoToolbox`, `Vulkan`.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Nvdec => "NVDEC",
            Kind::Vaapi => "VA-API",
            Kind::D3d11va => "Direct3D 11",
            Kind::VideoToolbox => "VideoToolbox",
            Kind::Vulkan => "Vulkan",
        }
    }

    pub(crate) fn device_type(self) -> ffi::AVHWDeviceType {
        use ffi::AVHWDeviceType as T;
        match self {
            Kind::Nvdec => T::AV_HWDEVICE_TYPE_CUDA,
            Kind::Vaapi => T::AV_HWDEVICE_TYPE_VAAPI,
            Kind::D3d11va => T::AV_HWDEVICE_TYPE_D3D11VA,
            Kind::VideoToolbox => T::AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
            Kind::Vulkan => T::AV_HWDEVICE_TYPE_VULKAN,
        }
    }

    /// Where its frames are.
    pub fn place(self) -> Place {
        match self {
            Kind::Nvdec => Place::Cuda,
            Kind::Vaapi => Place::Vaapi,
            Kind::D3d11va => Place::D3d11,
            Kind::VideoToolbox => Place::VideoToolbox,
            Kind::Vulkan => Place::Vulkan,
        }
    }

    /// The format of its frames: where their pictures are.
    pub(crate) fn pixel(self) -> format::Pixel {
        match self {
            Kind::Nvdec => format::Pixel::CUDA,
            Kind::Vaapi => format::Pixel::VAAPI,
            Kind::D3d11va => format::Pixel::D3D11,
            Kind::VideoToolbox => format::Pixel::VIDEOTOOLBOX,
            Kind::Vulkan => format::Pixel::VULKAN,
        }
    }
}

/// The codecs a GPU decoder decodes on Vulkan where its own decoder has
/// none for them, and FFmpeg has a Vulkan decoder (FFmpeg 8.1 and later
/// have ProRes's).
const ON_VULKAN: [ffmpeg::codec::Id; 1] = [ffmpeg::codec::Id::PRORES];

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
            return Err(Error::Ffmpeg {
                context: format!("opening {} on {on}", type_name(ty)),
                source: ffmpeg::Error::from(ret),
            });
        }
        Ok(Self(device))
    }

    /// FFmpeg's Vulkan device on `gpu`, for its Vulkan encoders: on
    /// NVIDIA's the one made from its CUDA device (FFmpeg matches their
    /// UUIDs), on Linux one made from its render node (their DRM nodes,
    /// else their PCI ids through VA-API), on Windows the one with its
    /// adapter's LUID.
    pub(crate) fn vulkan_on(gpu: &crate::gpus::Gpu) -> Result<Self> {
        if let Some(index) = gpu.cuda {
            return CudaDevice::on(index)?.vulkan();
        }
        let _one = making_vulkan();
        #[cfg(target_os = "linux")]
        if let Some(node) = &gpu.node {
            use ffi::AVHWDeviceType::{
                AV_HWDEVICE_TYPE_DRM, AV_HWDEVICE_TYPE_VAAPI, AV_HWDEVICE_TYPE_VULKAN,
            };
            return Self::of_type(AV_HWDEVICE_TYPE_DRM, node)
                .and_then(|drm| drm.derive(AV_HWDEVICE_TYPE_VULKAN))
                .or_else(|_| {
                    Self::of_type(AV_HWDEVICE_TYPE_VAAPI, node)?.derive(AV_HWDEVICE_TYPE_VULKAN)
                });
        }
        #[cfg(windows)]
        if let Some(luid) = gpu.adapter.and_then(crate::gpus::adapter_luid) {
            // FFmpeg takes Vulkan's devices by their index, in Vulkan's
            // order, which isn't DXGI's.
            for index in 0.. {
                let Ok(device) = Self::of_type(
                    ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VULKAN,
                    &index.to_string(),
                ) else {
                    break;
                };
                // SAFETY: a Vulkan device FFmpeg made.
                if unsafe { vulkan_luid(&device) } == Some(luid) {
                    return Ok(device);
                }
            }
        }
        Err(Error::Invalid(format!(
            "Vulkan doesn't list {} here",
            gpu.name
        )))
    }

    /// FFmpeg's device of type `ty` on the same GPU as this one.
    #[cfg(target_os = "linux")]
    fn derive(&self, ty: ffi::AVHWDeviceType) -> Result<Self> {
        let mut device = ptr::null_mut();
        // SAFETY: FFmpeg makes the device from this live one.
        let ret = unsafe { ffi::av_hwdevice_ctx_create_derived(&mut device, ty, self.0, 0) };
        if ret < 0 || device.is_null() {
            // SAFETY: a hardware device context's data.
            let from = unsafe { (*((*self.0).data as *const ffi::AVHWDeviceContext)).type_ };
            return Err(Error::Ffmpeg {
                context: format!("making {} from {}", type_name(ty), type_name(from)),
                source: ffmpeg::Error::from(ret),
            });
        }
        Ok(Self(device))
    }
}

/// FFmpeg's name for a type of device: `vulkan`, `cuda`.
fn type_name(ty: ffi::AVHWDeviceType) -> std::borrow::Cow<'static, str> {
    // SAFETY: FFmpeg's static name of a type, or null.
    unsafe {
        let n = ffi::av_hwdevice_get_type_name(ty);
        if n.is_null() {
            "a GPU device".into()
        } else {
            std::ffi::CStr::from_ptr(n).to_string_lossy()
        }
    }
}

/// Held while FFmpeg makes a Vulkan device: the Vulkan loader crashed when
/// two threads made instances at the same time (two exports at once).
pub(crate) fn making_vulkan() -> std::sync::MutexGuard<'static, ()> {
    static MAKING: Mutex<()> = Mutex::new(());
    MAKING.lock().unwrap_or_else(|e| e.into_inner())
}

/// The LUID of the GPU FFmpeg's Vulkan `device` is on, as Windows knows
/// its adapters.
///
/// # Safety
/// `device` is FFmpeg's Vulkan device.
#[cfg(windows)]
unsafe fn vulkan_luid(device: &Device) -> Option<[u8; 8]> {
    /// `VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2`.
    const PROPERTIES_2: i32 = 1_000_059_001;
    /// `VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_ID_PROPERTIES`.
    const ID_PROPERTIES: i32 = 1_000_071_004;
    /// `VkPhysicalDeviceIDProperties`.
    #[repr(C)]
    struct IdProperties {
        s_type: i32,
        next: *mut std::ffi::c_void,
        device_uuid: [u8; 16],
        driver_uuid: [u8; 16],
        device_luid: [u8; 8],
        node_mask: u32,
        luid_valid: u32,
    }
    /// `VkPhysicalDeviceProperties2`, room for its properties (824 bytes),
    /// which aren't read.
    #[repr(C)]
    struct Properties2 {
        s_type: i32,
        next: *mut std::ffi::c_void,
        properties: [u64; 128],
    }
    type GetProperties2 = unsafe extern "system" fn(ffi::VkPhysicalDevice, *mut Properties2);
    let mut id = IdProperties {
        s_type: ID_PROPERTIES,
        next: ptr::null_mut(),
        device_uuid: [0; 16],
        driver_uuid: [0; 16],
        device_luid: [0; 8],
        node_mask: 0,
        luid_valid: 0,
    };
    let mut properties = Properties2 {
        s_type: PROPERTIES_2,
        next: (&raw mut id).cast(),
        properties: [0; 128],
    };
    // SAFETY: a Vulkan device context, as the caller says; the call as
    // vulkan.h declares it, looked up through the device's loader.
    unsafe {
        let hw = &*((*device.0).data as *const ffi::AVHWDeviceContext);
        let dev = &*(hw.hwctx as *const ffi::AVVulkanDeviceContext);
        let get = dev.get_proc_addr?(dev.inst, c"vkGetPhysicalDeviceProperties2".as_ptr())?;
        let get: GetProperties2 = std::mem::transmute(get);
        get(dev.phys_dev, &mut properties);
    }
    (id.luid_valid != 0).then_some(id.device_luid)
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
    /// Its GPU's PCI vendor, where known: Direct3D 11's adapter's, and
    /// NVIDIA for NVDEC.
    vendor: Option<u32>,
    /// Its GPU, where it was opened for one (`gpus::list`).
    gpu: Option<crate::gpus::Gpu>,
}

impl GpuDecoder {
    /// NVDEC, on `cuda`.
    pub fn nvdec(cuda: CudaDevice) -> Self {
        Self {
            kind: Kind::Nvdec,
            device: Arc::new(Device(unsafe { ffi::av_buffer_ref(cuda.as_ptr()) })),
            on: String::new(),
            cuda: Some(cuda),
            vendor: Some(crate::gpus::NVIDIA),
            gpu: None,
        }
    }

    /// `kind` on `on` (a render node for VA-API, an adapter's index for
    /// Direct3D 11, a CUDA device's for NVDEC; the first one when empty).
    /// Vulkan comes with NVDEC, never alone.
    pub fn open(kind: Kind, on: &str) -> Result<Self> {
        if kind == Kind::Nvdec {
            return CudaDevice::on(on.parse().unwrap_or(0)).map(Self::nvdec);
        }
        if kind == Kind::Vulkan {
            return Err(Error::Invalid(
                "Vulkan decodes only beside NVDEC, which brings it along".into(),
            ));
        }
        let device = Device::open(kind, on)?;
        #[cfg(windows)]
        // SAFETY: a D3D11VA device FFmpeg made.
        let vendor = (kind == Kind::D3d11va)
            .then(|| unsafe { d3d11::desc(&device) }.map(|d| d.VendorId))
            .flatten();
        #[cfg(not(windows))]
        let vendor = None;
        Ok(Self {
            kind,
            device: Arc::new(device),
            on: on.to_string(),
            cuda: None,
            vendor,
            gpu: None,
        })
    }

    /// One of its own on the same GPU, for work beside the viewer's (an
    /// export, a proxy, the timeline's pictures): NVDEC on a CUDA stream of
    /// its own on the same context ([`CudaDevice::with_own_stream`]),
    /// VA-API and Direct3D 11 on a device of their own on the same render
    /// node or adapter. On the viewer's, its work would wait in line with
    /// playback's and hold playback up: CUDA's default stream runs every
    /// user's work in turn, and a Direct3D 11 device's context is one lock.
    pub fn own(&self) -> Result<Self> {
        match &self.cuda {
            Some(cuda) => Ok(Self::nvdec(cuda.with_own_stream()?)),
            None => Self::open(self.kind, &self.on),
        }
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Why it leaves `codec` to the CPU, when it does ([`why_not`]), on the
    /// kind of decoder that would decode it ([`kind_for`](Self::kind_for)).
    /// `open_gop` says whether the stream has open GOPs, asked only for
    /// H.264.
    pub fn why_not(
        &self,
        codec: ffmpeg::codec::Id,
        open_gop: impl FnOnce() -> bool,
    ) -> Option<&'static str> {
        why_not(self.kind_for(codec)?, self.vendor, codec, open_gop)
    }

    /// The kind of decoder it decodes `codec` with: its own where FFmpeg has
    /// a decoder for the codec there, else Vulkan for the codecs vidcut
    /// decodes there ([`ON_VULKAN`]) where FFmpeg has a Vulkan decoder for
    /// it. `None`: FFmpeg has neither.
    pub fn kind_for(&self, codec: ffmpeg::codec::Id) -> Option<Kind> {
        if crate::decode::gpu_decoder(codec, self.kind).is_some() {
            return Some(self.kind);
        }
        let vulkan =
            ON_VULKAN.contains(&codec) && crate::decode::gpu_decoder(codec, Kind::Vulkan).is_some();
        vulkan.then_some(Kind::Vulkan)
    }

    /// The device it decodes on with a decoder of `kind`: its own, or for
    /// Vulkan the one made from NVIDIA's CUDA device
    /// ([`CudaDevice::vulkan`]), asked for each time, so one dropped after
    /// a converter failed isn't used again.
    pub(crate) fn device_for(&self, kind: Kind) -> Result<Device> {
        match (kind, &self.cuda) {
            _ if kind == self.kind => Ok(Device::clone(&self.device)),
            (Kind::Vulkan, Some(cuda)) => cuda.vulkan(),
            _ => Err(Error::Invalid(format!(
                "{self} has no {} device",
                kind.label()
            ))),
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

    /// The GPU it was opened for (`gpus::list`); `None` for a Mac's, and
    /// for one opened by its kind and place, such as [`own`](Self::own)'s.
    pub fn gpu(&self) -> Option<&crate::gpus::Gpu> {
        self.gpu.as_ref()
    }

    pub(crate) fn as_ptr(&self) -> *mut ffi::AVBufferRef {
        self.device.0
    }

    /// The decoder on the GPU in use (`gpus::in_use`: the one the
    /// preferences chose, else the best this computer has), opened once for
    /// each; `VIDCUT_HWDEC` says otherwise. `None` without any.
    pub fn best() -> Option<Self> {
        static BEST: Mutex<Option<(String, Option<GpuDecoder>)>> = Mutex::new(None);
        let gpu = crate::gpus::in_use();
        let id = gpu.as_ref().map_or(String::new(), |g| g.id.clone());
        let mut best = BEST.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((was, decoder)) = best.as_ref()
            && *was == id
        {
            return decoder.clone();
        }
        let (decoder, how) = match std::env::var("VIDCUT_HWDEC") {
            Ok(asked) => Self::asked(&asked),
            Err(_) => match &gpu {
                Some(g) => match Self::on_gpu(g) {
                    Ok(d) => (Some(d), format!("{}, the GPU in use", g.id)),
                    Err(e) => (
                        None,
                        format!("{} ({}), the GPU in use, won't decode: {e}", g.name, g.id),
                    ),
                },
                None => Self::unlisted(),
            },
        };
        match &decoder {
            Some(d) => crate::debug!("decoding", "the GPU's decoder is {d}, {how}"),
            None => crate::debug!("decoding", "no GPU decoder: {how}; the CPU decodes"),
        }
        *best = Some((id, decoder.clone()));
        decoder
    }

    /// Every decoder this computer has: one on each GPU (`gpus::list`), or
    /// a Mac's.
    pub fn all() -> Vec<Self> {
        let listed = crate::gpus::list()
            .iter()
            .filter_map(|g| Self::on_gpu(g).ok());
        listed.chain(Self::unlisted().0).collect()
    }

    /// The decoder where no GPU is listed, and how it came: VideoToolbox on
    /// a Mac, whose one GPU decodes; none elsewhere.
    fn unlisted() -> (Option<Self>, String) {
        if !cfg!(target_os = "macos") {
            return (None, "no GPU is listed here".into());
        }
        match Self::open(Kind::VideoToolbox, "") {
            Ok(d) => (Some(d), "the Mac's".into()),
            Err(e) => (None, format!("VideoToolbox won't open: {e}")),
        }
    }

    /// The decoder on `gpu`: NVDEC on NVIDIA's CUDA device, Direct3D 11 on
    /// its adapter (Windows), VA-API on its render node (Linux).
    fn on_gpu(gpu: &crate::gpus::Gpu) -> Result<Self> {
        let named = |d: Self| Self {
            gpu: Some(gpu.clone()),
            ..d
        };
        if let Some(index) = gpu.cuda {
            return CudaDevice::on(index).map(Self::nvdec).map(named);
        }
        #[cfg(windows)]
        if let Some(adapter) = gpu.adapter {
            return Self::open(Kind::D3d11va, &adapter.to_string()).map(named);
        }
        #[cfg(target_os = "linux")]
        if let Some(node) = &gpu.node {
            return Self::open(Kind::Vaapi, node).map(named);
        }
        Err(Error::Invalid(format!("{} decodes nothing here", gpu.name)))
    }

    /// The decoder `VIDCUT_HWDEC` names (`kind[:on]`, or `none`), and how
    /// it came.
    fn asked(asked: &str) -> (Option<Self>, String) {
        let (kind, on) = asked.split_once(':').unwrap_or((asked, ""));
        match Kind::named(kind).map(|k| Self::open(k, on)) {
            Some(Ok(d)) => (Some(d), format!("as VIDCUT_HWDEC={asked} asks")),
            Some(Err(e)) => (None, format!("VIDCUT_HWDEC={asked} won't open: {e}")),
            None => (None, format!("VIDCUT_HWDEC={asked} asks for none")),
        }
    }
}

impl GpuDecoder {
    /// A decoder of `kind` on its GPU, for people: `Vulkan on NVIDIA
    /// GeForce RTX 4080 SUPER`.
    pub fn labeled(&self, kind: Kind) -> String {
        let on = match self.kind {
            _ if self.on.is_empty() => String::new(),
            Kind::D3d11va => format!("adapter {}", self.on),
            _ => self.on.clone(),
        };
        let label = kind.label();
        match (&self.gpu, on.is_empty()) {
            (Some(gpu), true) => format!("{label} on {}", gpu.name),
            (Some(gpu), false) => format!("{label} on {} ({on})", gpu.name),
            (None, true) => label.to_string(),
            (None, false) => format!("{label} on {on}"),
        }
    }
}

/// Its kind and where: `NVDEC on NVIDIA GeForce RTX 4080 SUPER`,
/// `Direct3D 11 on AMD Radeon(TM) Graphics (adapter 1)`, `VA-API on
/// /dev/dri/renderD128` where its GPU's name isn't known.
impl std::fmt::Display for GpuDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.labeled(self.kind))
    }
}

/// Why a GPU of `vendor` (its PCI vendor) leaves `codec` to the CPU on a
/// decoder of `kind`, when it does: it doesn't decode it right, as far as
/// known.
/// - AMD's Direct3D 11 decoder and VideoToolbox lose their way in H.264
///   sought into an open GOP (a keyframe that isn't IDR): AMD's shows one
///   picture over and over some frames on, VideoToolbox other frames than
///   the file's, where NVIDIA's and VA-API on Intel decode on. The CPU
///   decodes such H.264 there: `open_gop` says whether the stream has open
///   GOPs, asked only then.
/// - Vulkan decodes on NVIDIA's GPUs only: FFmpeg's ProRes decoder on Vulkan
///   crashes on AMD's Windows driver, and it is untested on others.
fn why_not(
    kind: Kind,
    vendor: Option<u32>,
    codec: ffmpeg::codec::Id,
    open_gop: impl FnOnce() -> bool,
) -> Option<&'static str> {
    let why = match (kind, vendor) {
        (Kind::Vulkan, Some(crate::gpus::NVIDIA)) => return None,
        (Kind::Vulkan, _) => return Some("vidcut decodes on Vulkan on NVIDIA's GPUs only"),
        (Kind::D3d11va, Some(crate::gpus::AMD)) => {
            "AMD's Direct3D 11 decoder loses its way after a seek in H.264 with open GOPs"
        }
        (Kind::VideoToolbox, _) => {
            "VideoToolbox shows other frames after a seek in H.264 with open GOPs"
        }
        _ => return None,
    };
    (codec == ffmpeg::codec::Id::H264 && open_gop()).then_some(why)
}

/// What a GPU decoder does with a test clip of one codec
/// ([`decoders`]).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DecoderStatus {
    /// The decoder and its GPU: `NVDEC on NVIDIA GeForce RTX 4080 SUPER`,
    /// `Vulkan on NVIDIA GeForce RTX 4080 SUPER` for ProRes there.
    pub decoder: String,
    /// Its kind: `nvdec`, `vaapi`, `d3d11va`, `videotoolbox`, `vulkan`.
    pub kind: &'static str,
    /// The clip's codec, and its bits where they aren't 8: `h264`,
    /// `hevc 10-bit`.
    pub codec: &'static str,
    pub ok: bool,
    /// Why not: why vidcut leaves the codec to the CPU there, or what
    /// failed.
    pub error: Option<String>,
}

/// What each GPU decoder here ([`GpuDecoder::all`]) decodes: a short clip
/// of each codec cameras and renders make, made in `dir` once and kept,
/// decoded through on each.
pub fn decoders(dir: &std::path::Path) -> Vec<DecoderStatus> {
    use crate::fixture::{Container, FixtureSpec, VideoCodec};
    use ffmpeg::codec::Id;
    crate::init();
    let clips = [
        ("h264", VideoCodec::H264, Container::Mp4, Id::H264),
        ("hevc 10-bit", VideoCodec::Hevc10, Container::Mp4, Id::HEVC),
        ("vp9", VideoCodec::Vp9, Container::Webm, Id::VP9),
        ("av1 10-bit", VideoCodec::Av1, Container::Mp4, Id::AV1),
        (
            "prores",
            VideoCodec::ProRes422Hq,
            Container::Mov,
            Id::PRORES,
        ),
    ]
    .map(|(codec, video, container, id)| {
        let spec = FixtureSpec::new(video, container, 640, 360, (30, 1), 8).no_audio();
        (codec, id, crate::fixture::ensure_in(dir, &spec))
    });
    let mut out = Vec::new();
    for gpu in GpuDecoder::all() {
        for (codec, id, clip) in &clips {
            let open_gop = || {
                clip.as_ref().is_ok_and(|p| {
                    crate::decode::h264_open_gop(&crate::decode::Input::File(p.clone()))
                })
            };
            let result = match (clip, gpu.why_not(*id, open_gop)) {
                (Err(e), _) => Err(format!("no test clip: {e}")),
                (Ok(_), Some(why)) => Err(format!("left to the CPU: {why}")),
                (Ok(path), None) => decodes_all(&gpu, path).map_err(|e| e.to_string()),
            };
            // Its decoder that decodes the codec: its own, or Vulkan.
            let kind = gpu.kind_for(*id).unwrap_or(gpu.kind());
            out.push(DecoderStatus {
                decoder: gpu.labeled(kind),
                kind: kind.name(),
                codec,
                ok: result.is_ok(),
                error: result.err(),
            });
        }
    }
    out
}

/// The clips [`plays`] decodes, one for each proxy codec by its name in the
/// preferences: [`decoders`]' clips. No GPU decodes DNxHR.
const PROXY_CLIPS: [(
    &str,
    crate::fixture::VideoCodec,
    crate::fixture::Container,
    ffmpeg::codec::Id,
); 4] = {
    use crate::fixture::{Container, VideoCodec};
    use ffmpeg::codec::Id;
    [
        ("h264", VideoCodec::H264, Container::Mp4, Id::H264),
        ("av1", VideoCodec::Av1, Container::Mp4, Id::AV1),
        ("hevc", VideoCodec::Hevc10, Container::Mp4, Id::HEVC),
        (
            "prores",
            VideoCodec::ProRes422Hq,
            Container::Mov,
            Id::PRORES,
        ),
    ]
};

/// The proxy codecs [`plays`] tests: `h264`, `av1`, `hevc`, `prores`.
pub fn plays_tests() -> Vec<&'static str> {
    PROXY_CLIPS.iter().map(|&(codec, ..)| codec).collect()
}

/// The proxy codecs `gpu` decodes, of those [`plays_tests`] names, each with
/// the kind of its decoders that decodes it ([`GpuDecoder::kind_for`]:
/// Vulkan for ProRes on NVIDIA's GPU): a short clip of each, made in `dir`
/// once and kept, decoded through on it.
pub fn plays(gpu: &GpuDecoder, dir: &std::path::Path) -> Vec<(&'static str, Kind)> {
    use crate::fixture::FixtureSpec;
    crate::init();
    PROXY_CLIPS
        .into_iter()
        .filter(|&(_, video, container, id)| {
            let spec = FixtureSpec::new(video, container, 640, 360, (30, 1), 8).no_audio();
            // Proxies close their GOPs.
            crate::fixture::ensure_in(dir, &spec).is_ok_and(|clip| {
                gpu.why_not(id, || false).is_none() && decodes_all(gpu, &clip).is_ok()
            })
        })
        .map(|(codec, .., id)| (codec, gpu.kind_for(id).unwrap_or(gpu.kind())))
        .collect()
}

/// Decodes all 8 frames of the clip at `path` on `gpu`.
fn decodes_all(gpu: &GpuDecoder, path: &std::path::Path) -> Result<()> {
    let input = crate::decode::Input::File(path.to_path_buf());
    let mut reader = crate::decode::VideoReader::open_on(&input, gpu)?;
    let mut frames = 0;
    while reader.next_frame()?.is_some() {
        frames += 1;
    }
    if frames == 8 {
        Ok(())
    } else {
        Err(Error::Invalid(format!("it gave {frames} of 8 frames")))
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
    let f = downloaded(src)?;
    crate::copies::down(&f);
    Ok(f)
}

/// [`download`], uncounted.
fn downloaded(src: &frame::Video) -> Result<frame::Video> {
    vaapi::intel_download(src).unwrap_or_else(|| fetched(src))
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

/// Keeps the GPU's frames there without running its decoder out of
/// surfaces: Direct3D 11's are copied into textures of their own that
/// another device can open (Windows). FFmpeg copies NVDEC's itself, and
/// VA-API's, VideoToolbox's and Vulkan's pools grow as they need. Vulkan's
/// are kept once decoded ([`vulkan::keep`]).
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
    /// `f`, a frame in GPU memory, to keep there, with its timestamp and
    /// colors; `keeper` made for frames like it first.
    pub(crate) fn keep(keeper: &mut Option<Keeper>, f: &frame::Video) -> Result<frame::Video> {
        match Kind::of_frame(f) {
            Some(Kind::Nvdec | Kind::Vaapi | Kind::VideoToolbox) => {
                crate::export::video::frame_ref(f)
            }
            Some(Kind::Vulkan) => vulkan::keep(f),
            #[cfg(windows)]
            Some(Kind::D3d11va) => {
                let (frames, like) = frames_of(f)?;
                if !matches!(keeper, Some(Keeper::D3d11(k)) if k.like == like) {
                    *keeper = Some(Keeper::D3d11(d3d11::Keeper::new(f, frames, like)?));
                }
                let Some(Keeper::D3d11(k)) = keeper else {
                    unreachable!("made above")
                };
                k.copy(f)
            }
            _ => {
                // Only Direct3D 11's take a keeper.
                let _ = keeper;
                Err(Error::Invalid(format!(
                    "no keeping {:?} frames",
                    f.format()
                )))
            }
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
    crate::parts::parse_filters(&mut graph, spec)?;
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

/// Scales frames down on the GPU (VA-API's, Direct3D 11's, CUDA's and
/// VideoToolbox's), so they come down to memory small, in the frames' own
/// layout (NV12, P010).
pub(crate) struct Shrinker {
    /// `None`: the GPU won't scale these frames, so they come down whole.
    scaling: Option<Scaling>,
    /// The frames context, layout and frame size it was made for, and the
    /// size it scales to.
    input: (KeptLike, (u32, u32)),
}

enum Scaling {
    /// `scale_vaapi`, `scale_vt`, or `scale_cuda` in steps.
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
                let spec = vaapi::shrink_spec((w, h));
                Ok(Self::Filters(gpu_graph(f, frames, pixel, &spec)?))
            }
            format::Pixel::CUDA => {
                let pixel = ffi::AVPixelFormat::AV_PIX_FMT_CUDA;
                let spec = crate::cuda::scaling((f.width(), f.height()), (w, h), "").join(",");
                Ok(Self::Filters(gpu_graph(f, frames, pixel, &spec)?))
            }
            format::Pixel::VIDEOTOOLBOX => {
                let pixel = ffi::AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX;
                let spec = vt::shrink_spec((w, h));
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
            let scaling = Scaling::new(f, frames, size)
                .inspect_err(|e| {
                    crate::debug!(
                        "decoding",
                        "{:?} frames {}x{} come down whole, not shrunk to {}x{} on the GPU: {e}",
                        sw_format(f),
                        f.width(),
                        f.height(),
                        size.0,
                        size.1
                    )
                })
                .ok();
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
            Err(e) => {
                crate::debug!(
                    "decoding",
                    "shrinking on the GPU failed, frames come down whole from now on: {e}"
                );
                shrinker.scaling = None;
                download(f)
            }
        }
    }
}

/// A frames context in GPU memory, counted.
pub(crate) struct Frames(*mut ffi::AVBufferRef);

// SAFETY: FFmpeg's frames contexts are reference-counted and may be used
// on any thread.
unsafe impl Send for Frames {}

impl Frames {
    /// Another reference to `frames`, a live frames context.
    ///
    /// # Safety
    /// `frames` is a live buffer reference.
    pub(crate) unsafe fn of(frames: *mut ffi::AVBufferRef) -> Result<Self> {
        // SAFETY: as the caller says.
        let frames = unsafe { ffi::av_buffer_ref(frames) };
        if frames.is_null() {
            return Err(Error::Invalid("no memory for frames".into()));
        }
        Ok(Self(frames))
    }

    #[cfg(windows)]
    pub(crate) fn as_ptr(&self) -> *mut ffi::AVBufferRef {
        self.0
    }
}

impl Drop for Frames {
    fn drop(&mut self) {
        // SAFETY: its own reference.
        unsafe { ffi::av_buffer_unref(&mut self.0) };
    }
}

/// Fits frames into an output picture on the GPU, for an encoder on their
/// device: turned upright, scaled to the size they are shown at, into the
/// output's layout, and centered on black where their aspect differs, at
/// even offsets as the CPU pads. VA-API's with `transpose_vaapi`,
/// `scale_vaapi` and `pad_vaapi`; Direct3D 11's on the video processor,
/// into the encoder's own frames (AMF takes those alone); VideoToolbox's
/// with `transpose_vt` and `scale_vt`, which keep their layout and place
/// nothing on black.
pub(crate) struct Fitter {
    fitting: Fitting,
    /// The frames context, layout and frame size it was made for, the size
    /// it scales to, and the turn.
    input: (KeptLike, (u32, u32), u16),
}

enum Fitting {
    Filters(ffmpeg::filter::Graph),
    #[cfg(windows)]
    D3d11(d3d11::Scaler),
}

/// Whether a frame's GPU (`place`) fits it into the output as `ops` says,
/// for an encoder on its device. Each backend says for its own frames.
/// Vulkan's fit as libplacebo fits them, as CUDA's do. Frames in memory
/// never fit.
pub(crate) fn can_fit(place: Place, ops: &Ops) -> bool {
    match place {
        Place::Cuda | Place::Vulkan => crate::cuda::can_fit(ops),
        Place::Vaapi => vaapi::can_fit(ops),
        #[cfg(windows)]
        Place::D3d11 => d3d11::can_fit(ops),
        Place::VideoToolbox => vt::can_fit(ops),
        _ => false,
    }
}

/// Whether pictures on `place` are mixed there for a dissolve (decision
/// D50): CUDA's ([`crate::cuda::mix`]). Others come down to be mixed.
pub(crate) fn can_mix(place: Place) -> bool {
    place == Place::Cuda
}

/// Whether a frame's GPU (`from`) hands the picture it fits to an encoder
/// that takes frames on another place, `to`: CUDA's to Vulkan's encoder,
/// Vulkan's to NVENC's CUDA frames ([`crate::cuda::reaches`]). No other
/// GPU's frames leave their place.
pub(crate) fn reaches(from: Place, to: Place) -> bool {
    match from {
        Place::Cuda | Place::Vulkan => crate::cuda::reaches(from, to),
        _ => false,
    }
}

impl Fitter {
    /// `f` turned clockwise by `rotation` (0, 90, 180 or 270), scaled to
    /// `fitted` and placed in an `out` picture of `layout` (NV12, P010), on
    /// the GPU, with its timestamp and colors, its pixels square; `fitter`
    /// made for frames like it first. Direct3D 11's go into `into`, the
    /// encoder's frames. VideoToolbox's fit only where they fill the
    /// picture in their own layout.
    pub(crate) fn fit(
        fitter: &mut Option<Fitter>,
        f: &frame::Video,
        rotation: u16,
        fitted: (u32, u32),
        (w, h): (u32, u32),
        layout: format::Pixel,
        into: Option<&Frames>,
    ) -> Result<frame::Video> {
        let (frames, of) = frames_of(f)?;
        let input = (of, fitted, rotation);
        if fitter.as_ref().is_none_or(|x| x.input != input) {
            let fitting = match f.format() {
                format::Pixel::VAAPI => {
                    let spec = vaapi::fit_spec(rotation, fitted, (w, h), layout);
                    let pixel = ffi::AVPixelFormat::AV_PIX_FMT_VAAPI;
                    Fitting::Filters(gpu_graph(f, frames, pixel, &spec)?)
                }
                // The planner asks for neither (`vt::can_fit`).
                format::Pixel::VIDEOTOOLBOX => {
                    if fitted != (w, h) {
                        return Err(Error::Invalid(
                            "VideoToolbox doesn't place pictures on black".into(),
                        ));
                    }
                    let from = sw_format(f);
                    if from != layout {
                        return Err(Error::Invalid(format!(
                            "VideoToolbox doesn't change {from:?} frames into {layout:?}"
                        )));
                    }
                    let spec = vt::fit_spec(rotation, fitted);
                    let pixel = ffi::AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX;
                    Fitting::Filters(gpu_graph(f, frames, pixel, &spec)?)
                }
                #[cfg(windows)]
                format::Pixel::D3D11 => {
                    let into = into.ok_or_else(|| {
                        Error::Invalid("no frames to fit Direct3D 11's into".into())
                    })?;
                    // SAFETY: the encoder's frames, on the decoder's device.
                    Fitting::D3d11(unsafe {
                        d3d11::Scaler::into(f, frames, into.as_ptr(), fitted, rotation)?
                    })
                }
                other => return Err(Error::Invalid(format!("no fitting {other:?} frames"))),
            };
            *fitter = Some(Self { fitting, input });
        }
        let _ = into;
        let mut out = match &mut fitter.as_mut().expect("made above").fitting {
            Fitting::Filters(graph) => run_graph(graph, f, "fitting a frame on the GPU")?,
            #[cfg(windows)]
            Fitting::D3d11(scaler) => scaler.scale(f)?,
        };
        same_picture(&mut out, f);
        // SAFETY: a plain field of its own frame.
        unsafe { (*out.as_mut_ptr()).sample_aspect_ratio = ffi::AVRational { num: 1, den: 1 } };
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffmpeg::codec::Id;
    use crate::gpus::{AMD, INTEL, NVIDIA};

    #[test]
    fn nvidias_gpu_decodes_prores_on_vulkan() {
        assert_eq!(
            why_not(Kind::Vulkan, Some(NVIDIA), Id::PRORES, || false),
            None
        );
    }

    #[test]
    fn amds_gpu_leaves_prores_on_vulkan_to_the_cpu() {
        assert!(why_not(Kind::Vulkan, Some(AMD), Id::PRORES, || false).is_some());
    }

    #[test]
    fn intels_gpu_leaves_prores_on_vulkan_to_the_cpu() {
        assert!(why_not(Kind::Vulkan, Some(INTEL), Id::PRORES, || false).is_some());
    }

    #[test]
    fn a_gpu_of_unknown_make_leaves_prores_on_vulkan_to_the_cpu() {
        assert!(why_not(Kind::Vulkan, None, Id::PRORES, || false).is_some());
    }

    #[test]
    fn vulkans_frames_are_at_vulkans_place() {
        assert_eq!(Kind::Vulkan.place(), Place::Vulkan);
    }

    #[test]
    fn vulkan_is_named_vulkan() {
        assert_eq!(Kind::named("vulkan"), Some(Kind::Vulkan));
    }

    #[test]
    fn the_label_vulkan_is_vulkans() {
        assert_eq!(Kind::with_label("Vulkan"), Some(Kind::Vulkan));
    }

    #[test]
    fn vulkan_is_labeled_vulkan() {
        assert_eq!(Kind::Vulkan.label(), "Vulkan");
    }

    #[test]
    fn a_vulkan_frame_turned_a_quarter_fits() {
        let ops = Ops {
            rotation: 90,
            layout: format::Pixel::YUV422P10LE,
            out_layout: format::Pixel::YUV422P10LE,
            rgb: false,
            colors_kept: true,
            pads: false,
            into: false,
        };
        assert!(can_fit(Place::Vulkan, &ops));
    }

    #[test]
    fn cudas_frames_reach_vulkans_encoder() {
        assert!(reaches(Place::Cuda, Place::Vulkan));
    }

    #[test]
    fn cudas_frames_reach_no_other_gpus_encoder() {
        for to in [Place::Vaapi, Place::D3d11, Place::VideoToolbox] {
            assert!(!reaches(Place::Cuda, to), "{to:?}");
        }
    }

    #[test]
    fn vulkans_frames_reach_nvencs_cuda_frames() {
        assert!(reaches(Place::Vulkan, Place::Cuda));
    }

    #[test]
    fn vulkans_frames_reach_no_other_gpus_encoder() {
        for to in [Place::Vaapi, Place::D3d11, Place::VideoToolbox] {
            assert!(!reaches(Place::Vulkan, to), "{to:?}");
        }
    }

    #[test]
    fn other_gpus_frames_reach_no_other_place() {
        let gpus = [
            Place::Cuda,
            Place::Vaapi,
            Place::D3d11,
            Place::VideoToolbox,
            Place::Vulkan,
        ];
        let others = [Place::Vaapi, Place::D3d11, Place::VideoToolbox];
        for from in others {
            for to in gpus.into_iter().filter(|&to| to != from) {
                assert!(!reaches(from, to), "{from:?} to {to:?}");
            }
        }
    }
}
