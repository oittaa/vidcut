#[cfg(windows)]
pub mod d3d11;
pub(crate) mod deinterlace;
mod gpu_graph;
pub mod vaapi;
pub(crate) mod vt;
pub mod vulkan;
mod vulkan_scale;

pub(crate) use gpu_graph::GpuGraph;

use std::ptr;
use std::sync::{Arc, Mutex};

use crate::cuda::CudaDevice;
use crate::error::{Error, Result};
use crate::ffmpeg::{self, ffi, format, frame};
use crate::route::{Ops, Place};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Nvdec,
    Vaapi,
    D3d11va,
    VideoToolbox,
    Vulkan,
}

impl Kind {
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

    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    pub fn with_label(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.label() == label)
    }

    pub fn of_frame(f: &frame::Video) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.pixel() == f.format())
    }

    pub(crate) unsafe fn of_device(device: *const ffi::AVBufferRef) -> Option<Self> {
        if device.is_null() {
            return None;
        }
        // SAFETY: a hardware device context's data, as the caller says.
        let ty = unsafe { (*((*device).data as *const ffi::AVHWDeviceContext)).type_ };
        Self::ALL.into_iter().find(|k| k.device_type() == ty)
    }

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

    pub fn place(self) -> Place {
        match self {
            Kind::Nvdec => Place::Cuda,
            Kind::Vaapi => Place::Vaapi,
            Kind::D3d11va => Place::D3d11,
            Kind::VideoToolbox => Place::VideoToolbox,
            Kind::Vulkan => Place::Vulkan,
        }
    }

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

const ON_VULKAN: [ffmpeg::codec::Id; 1] = [ffmpeg::codec::Id::PRORES];

pub(crate) struct Device(*mut ffi::AVBufferRef);

// SAFETY: FFmpeg counts the references atomically, so any thread may clone, use or drop one.
unsafe impl Send for Device {}
unsafe impl Sync for Device {}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe { ffi::av_buffer_unref(&mut self.0) };
    }
}

impl Device {
    pub(crate) unsafe fn from_raw(ptr: *mut ffi::AVBufferRef) -> Self {
        Self(ptr)
    }

    pub(crate) fn as_ptr(&self) -> *mut ffi::AVBufferRef {
        self.0
    }

    pub(crate) fn clone(&self) -> Result<Self> {
        let buf = unsafe { ffi::av_buffer_ref(self.0) };
        if buf.is_null() {
            return Err(Error::Invalid("no memory for a GPU device".into()));
        }
        Ok(Self(buf))
    }

    fn open(kind: Kind, on: &str) -> Result<Self> {
        Self::of_type(kind.device_type(), on)
    }

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
        // SAFETY: FFmpeg only reads `raw`, the options' dictionary, which is freed after the call.
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

pub(crate) fn making_vulkan() -> std::sync::MutexGuard<'static, ()> {
    static MAKING: Mutex<()> = Mutex::new(());
    MAKING.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(windows)]
unsafe fn vulkan_luid(device: &Device) -> Option<[u8; 8]> {
    const PROPERTIES_2: i32 = 1_000_059_001;
    const ID_PROPERTIES: i32 = 1_000_071_004;
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
    // SAFETY: `device` is a Vulkan device context; `get` is its loader's, typed as vulkan.h has it.
    unsafe {
        let hw = &*((*device.0).data as *const ffi::AVHWDeviceContext);
        let dev = &*(hw.hwctx as *const ffi::AVVulkanDeviceContext);
        let get = dev.get_proc_addr?(dev.inst, c"vkGetPhysicalDeviceProperties2".as_ptr())?;
        let get: GetProperties2 = std::mem::transmute(get);
        get(dev.phys_dev, &mut properties);
    }
    (id.luid_valid != 0).then_some(id.device_luid)
}

#[derive(Clone)]
pub struct GpuDecoder {
    kind: Kind,
    device: Arc<Device>,
    on: String,
    cuda: Option<CudaDevice>,
    vendor: Option<u32>,
    gpu: Option<crate::gpus::Gpu>,
}

impl GpuDecoder {
    pub fn nvdec(cuda: CudaDevice) -> Result<Self> {
        let device = unsafe { ffi::av_buffer_ref(cuda.as_ptr()) };
        if device.is_null() {
            return Err(Error::Invalid("no memory for a GPU device".into()));
        }
        Ok(Self {
            kind: Kind::Nvdec,
            device: Arc::new(Device(device)),
            on: String::new(),
            cuda: Some(cuda),
            vendor: Some(crate::gpus::NVIDIA),
            gpu: None,
        })
    }

    pub fn open(kind: Kind, on: &str) -> Result<Self> {
        if kind == Kind::Nvdec {
            return CudaDevice::on(on.parse().unwrap_or(0)).and_then(Self::nvdec);
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

    pub fn own(&self) -> Result<Self> {
        match &self.cuda {
            Some(cuda) => Self::nvdec(cuda.with_own_stream()?),
            None => Self::open(self.kind, &self.on),
        }
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn why_not(
        &self,
        codec: ffmpeg::codec::Id,
        open_gop: impl FnOnce() -> bool,
    ) -> Option<&'static str> {
        why_not(self.kind_for(codec)?, self.vendor, codec, open_gop)
    }

    pub fn kind_for(&self, codec: ffmpeg::codec::Id) -> Option<Kind> {
        if crate::decode::gpu_decoder(codec, self.kind).is_some() {
            return Some(self.kind);
        }
        let vulkan =
            ON_VULKAN.contains(&codec) && crate::decode::gpu_decoder(codec, Kind::Vulkan).is_some();
        vulkan.then_some(Kind::Vulkan)
    }

    pub(crate) fn device_for(&self, kind: Kind) -> Result<Device> {
        match (kind, &self.cuda) {
            _ if kind == self.kind => self.device.as_ref().clone(),
            (Kind::Vulkan, Some(cuda)) => cuda.vulkan(),
            _ => Err(Error::Invalid(format!(
                "{self} has no {} device",
                kind.label()
            ))),
        }
    }

    pub fn on(&self) -> &str {
        &self.on
    }

    pub fn cuda(&self) -> Option<&CudaDevice> {
        self.cuda.as_ref()
    }

    pub fn gpu(&self) -> Option<&crate::gpus::Gpu> {
        self.gpu.as_ref()
    }

    pub(crate) fn as_ptr(&self) -> *mut ffi::AVBufferRef {
        self.device.0
    }

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

    pub fn all() -> Vec<Self> {
        let listed = crate::gpus::list()
            .iter()
            .filter_map(|g| Self::on_gpu(g).ok());
        listed.chain(Self::unlisted().0).collect()
    }

    fn unlisted() -> (Option<Self>, String) {
        if !cfg!(target_os = "macos") {
            return (None, "no GPU is listed here".into());
        }
        match Self::open(Kind::VideoToolbox, "") {
            Ok(d) => (Some(d), "the Mac's".into()),
            Err(e) => (None, format!("VideoToolbox won't open: {e}")),
        }
    }

    fn on_gpu(gpu: &crate::gpus::Gpu) -> Result<Self> {
        let named = |d: Self| Self {
            gpu: Some(gpu.clone()),
            ..d
        };
        if let Some(index) = gpu.cuda {
            return CudaDevice::on(index).and_then(Self::nvdec).map(named);
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

impl std::fmt::Display for GpuDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.labeled(self.kind))
    }
}

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

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DecoderStatus {
    pub decoder: String,
    pub kind: &'static str,
    pub codec: &'static str,
    pub ok: bool,
    pub error: Option<String>,
}

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
    for (_, _, clip) in &clips {
        if let Ok(path) = clip {
            vidcut_core::disk_cache::added(path);
        }
    }
    out
}

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

pub fn plays_tests() -> Vec<&'static str> {
    PROXY_CLIPS.iter().map(|&(codec, ..)| codec).collect()
}

pub fn plays(gpu: &GpuDecoder, dir: &std::path::Path) -> Vec<(&'static str, Kind)> {
    use crate::fixture::FixtureSpec;
    crate::init();
    PROXY_CLIPS
        .into_iter()
        .filter(|&(_, video, container, id)| {
            let spec = FixtureSpec::new(video, container, 640, 360, (30, 1), 8).no_audio();
            crate::fixture::ensure_in(dir, &spec).is_ok_and(|clip| {
                let plays = gpu.why_not(id, || false).is_none() && decodes_all(gpu, &clip).is_ok();
                vidcut_core::disk_cache::added(&clip);
                plays
            })
        })
        .map(|(codec, .., id)| (codec, gpu.kind_for(id).unwrap_or(gpu.kind())))
        .collect()
}

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

pub(crate) fn pooled(frame: &frame::Video) -> Option<usize> {
    match Kind::of_frame(frame) {
        #[cfg(target_os = "linux")]
        Some(Kind::Vaapi) => vaapi::pooled(frame),
        // SAFETY: reads the frame's own field, a live buffer reference.
        Some(Kind::Vulkan) => unsafe {
            let frames = (*frame.as_ptr()).hw_frames_ctx;
            (!frames.is_null()).then(|| (*frames).data as usize)
        },
        _ => None,
    }
}

pub(crate) fn surface_bytes(frame: &frame::Video) -> Option<usize> {
    match Kind::of_frame(frame) {
        #[cfg(target_os = "linux")]
        Some(Kind::Vaapi) => vaapi::surface_bytes(frame),
        Some(Kind::Vulkan) => vulkan::image_bytes(frame),
        _ => None,
    }
}

pub(crate) fn let_unused_frames_go() {
    #[cfg(target_os = "linux")]
    vaapi::let_unused_frames_go();
}

pub fn on_gpu(frame: &frame::Video) -> bool {
    // SAFETY: reads the frame's own field.
    unsafe { !(*frame.as_ptr()).hw_frames_ctx.is_null() }
}

pub(crate) fn sw_format(frame: &frame::Video) -> format::Pixel {
    // SAFETY: a non-null `hw_frames_ctx` is the frame's reference to an `AVHWFramesContext`.
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

pub fn download(src: &frame::Video) -> Result<frame::Video> {
    let f = downloaded(src)?;
    crate::copies::down(&f);
    Ok(f)
}

pub fn to_read(src: &frame::Video) -> Result<frame::Video> {
    match Kind::of_frame(src) {
        Some(Kind::VideoToolbox) => vt::mapped(src),
        _ => download(src),
    }
}

fn downloaded(src: &frame::Video) -> Result<frame::Video> {
    vaapi::intel_download(src).unwrap_or_else(|| fetched(src))
}

fn fetched(src: &frame::Video) -> Result<frame::Video> {
    let mut dst = frame::Video::empty();
    // SAFETY: both frames are valid, and the transfer allocates the empty `dst`.
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

pub(crate) enum Keeper {
    Nvdec(Option<crate::cuda::keep::Keeper>),
    #[cfg(target_os = "linux")]
    Vaapi(Option<vaapi::Keeper>),
    #[cfg(windows)]
    D3d11(d3d11::Keeper),
}

type KeptLike = (usize, format::Pixel, u32, u32);

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
    pub(crate) fn keep(keeper: &mut Option<Keeper>, f: &frame::Video) -> Result<frame::Video> {
        match Kind::of_frame(f) {
            Some(Kind::Nvdec) => {
                let made = matches!(keeper, Some(Keeper::Nvdec(k))
                    if k.as_ref().is_none_or(|k| k.is_for(f)));
                if !made {
                    *keeper = Some(Keeper::Nvdec(crate::cuda::keep::Keeper::new(f)?));
                }
                let Some(Keeper::Nvdec(Some(k))) = keeper else {
                    return crate::export::video::frame_ref(f);
                };
                let mut kept = k.copy(f)?;
                props(&mut kept, f)?;
                Ok(kept)
            }
            #[cfg(target_os = "linux")]
            Some(Kind::Vaapi) => {
                let_unused_frames_go();
                let made = matches!(keeper, Some(Keeper::Vaapi(k))
                    if k.as_ref().is_none_or(|k| k.is_for(f)));
                if !made {
                    *keeper = Some(Keeper::Vaapi(vaapi::Keeper::new(f)?));
                }
                let Some(Keeper::Vaapi(Some(k))) = keeper else {
                    return crate::export::video::frame_ref(f);
                };
                match k.copy(f)? {
                    Some(mut kept) => {
                        props(&mut kept, f)?;
                        Ok(kept)
                    }
                    None => {
                        *keeper = Some(Keeper::Vaapi(None));
                        crate::export::video::frame_ref(f)
                    }
                }
            }
            #[cfg(not(target_os = "linux"))]
            Some(Kind::Vaapi) => crate::export::video::frame_ref(f),
            Some(Kind::VideoToolbox) => crate::export::video::frame_ref(f),
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
                let _ = keeper;
                Err(Error::Invalid(format!(
                    "no keeping {:?} frames",
                    f.format()
                )))
            }
        }
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

fn gpu_graph(
    f: &frame::Video,
    frames: *mut ffi::AVBufferRef,
    pixel: ffi::AVPixelFormat,
    spec: &str,
) -> Result<GpuGraph> {
    let mut graph = GpuGraph::new();
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
    graph.parse_filters(spec)?;
    graph
        .validate()
        .map_err(failed(format!("configuring {spec}")))?;
    Ok(graph)
}

fn run_graph(graph: &mut GpuGraph, f: &frame::Video, doing: &str) -> Result<frame::Video> {
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

pub(crate) struct Shrinker {
    scaling: Option<Scaling>,
    input: (KeptLike, (u32, u32)),
}

enum Scaling {
    Filters(GpuGraph),
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
            format::Pixel::VULKAN => Ok(Self::Filters(vulkan_scale::scaler(f, frames, (w, h))?)),
            #[cfg(windows)]
            format::Pixel::D3D11 => Ok(Self::D3d11(d3d11::Scaler::new(f, frames, (w, h))?)),
            other => Err(Error::Invalid(format!("no scaling {other:?} frames"))),
        }
    }
}

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

fn readable((w, h): (u32, u32)) -> bool {
    w as usize >= 4 * BANDS.len() && h >= 8
}

fn described(layout: format::Pixel) -> Option<&'static ffi::AVPixFmtDescriptor> {
    // SAFETY: FFmpeg's static description, or null.
    let desc = unsafe { ffi::av_pix_fmt_desc_get(layout.into()).as_ref()? };
    let odd = ffi::AV_PIX_FMT_FLAG_FLOAT
        | ffi::AV_PIX_FMT_FLAG_PAL
        | ffi::AV_PIX_FMT_FLAG_BAYER
        | ffi::AV_PIX_FMT_FLAG_BITSTREAM
        | ffi::AV_PIX_FMT_FLAG_HWACCEL;
    let samples = &desc.comp[..usize::from(desc.nb_components)];
    (desc.flags & odd as u64 == 0 && samples.iter().all(|s| s.depth >= 8)).then_some(desc)
}

fn component_size(desc: &ffi::AVPixFmtDescriptor, c: i32, (w, h): (u32, u32)) -> (usize, usize) {
    let rgb = desc.flags & ffi::AV_PIX_FMT_FLAG_RGB as u64 != 0;
    let chroma = !rgb && desc.nb_components >= 3 && matches!(c, 1 | 2);
    let (sw, sh) = if chroma {
        (desc.log2_chroma_w, desc.log2_chroma_h)
    } else {
        (0, 0)
    };
    (w.div_ceil(1 << sw) as usize, h.div_ceil(1 << sh) as usize)
}

fn level(desc: &ffi::AVPixFmtDescriptor, c: i32, color: [u16; 3]) -> u16 {
    let column = if desc.nb_components >= 3 && c < 3 {
        c as usize
    } else {
        0
    };
    color[column] << (desc.comp[c as usize].depth - 8)
}

fn blank(layout: format::Pixel, (w, h): (u32, u32)) -> frame::Video {
    let mut picture = frame::Video::new(layout, w, h);
    for p in 0..picture.planes() {
        picture.data_mut(p).fill(0);
    }
    picture
}

fn write_line(picture: &mut frame::Video, c: i32, y: usize, line: &[u16]) {
    let desc = described(picture.format()).expect("a layout test pictures come in");
    let (w, h) = component_size(desc, c, (picture.width(), picture.height()));
    assert!(
        (0..i32::from(desc.nb_components)).contains(&c) && y < h && line.len() <= w,
        "the picture has no row {y} of {} samples in component {c}",
        line.len()
    );
    // SAFETY: the assert keeps row `y` and `line.len()` samples of component `c` in the picture.
    unsafe {
        let raw = picture.as_mut_ptr();
        ffi::av_write_image_line2(
            line.as_ptr().cast(),
            (*raw).data.as_mut_ptr(),
            (*raw).linesize.as_ptr(),
            desc,
            0,
            y as i32,
            c,
            line.len() as i32,
            2,
        );
    }
}

impl Shrinker {
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

pub(crate) struct Frames(*mut ffi::AVBufferRef);

// SAFETY: FFmpeg's frames contexts are reference-counted and may be used on any thread.
unsafe impl Send for Frames {}

impl Frames {
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

const FITTERS: usize = 2;

pub(crate) struct Fitter {
    fitting: Fitting,
    input: (KeptLike, (u32, u32), u16),
}

enum Fitting {
    Filters(GpuGraph),
    #[cfg(windows)]
    D3d11(d3d11::Scaler),
}

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

pub(crate) fn can_mix(place: Place) -> bool {
    place == Place::Cuda
}

pub(crate) fn reaches(from: Place, to: Place) -> bool {
    match from {
        Place::Cuda | Place::Vulkan => crate::cuda::reaches(from, to),
        _ => false,
    }
}

impl Fitter {
    pub(crate) fn fit(
        fitters: &mut Vec<Fitter>,
        f: &frame::Video,
        rotation: u16,
        fitted: (u32, u32),
        (w, h): (u32, u32),
        layout: format::Pixel,
        into: Option<&Frames>,
    ) -> Result<frame::Video> {
        let (frames, of) = frames_of(f)?;
        let input = (of, fitted, rotation);
        if !fitters.iter().any(|x| x.input == input) {
            let fitting = match f.format() {
                format::Pixel::VAAPI => {
                    let spec = vaapi::fit_spec(rotation, fitted, (w, h), layout);
                    let pixel = ffi::AVPixelFormat::AV_PIX_FMT_VAAPI;
                    Fitting::Filters(gpu_graph(f, frames, pixel, &spec)?)
                }
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
            if fitters.len() == FITTERS {
                fitters.remove(0);
            }
            fitters.push(Self { fitting, input });
        }
        let _ = into;
        let fitter = fitters.iter_mut().find(|x| x.input == input);
        let mut out = match &mut fitter.expect("made above").fitting {
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
