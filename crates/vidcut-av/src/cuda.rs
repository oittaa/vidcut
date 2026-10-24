#![warn(clippy::undocumented_unsafe_blocks)]

pub(crate) mod burn;
pub(crate) mod keep;
pub(crate) mod mix;
pub mod share;

pub(crate) use share::{Listed, listed};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaneCopy {
    pub offset: u64,
    pub pitch: u64,
    pub row_bytes: u64,
    pub rows: u64,
}

use std::collections::VecDeque;
use std::path::PathBuf;
use std::ptr;
use std::sync::{Arc, Mutex, OnceLock};

use vidcut_core::model::{Hdr, Sharpness};

use crate::color::{GpuColor, Tags};
use crate::decode::VideoReader;
use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, ffi, filter, format, frame};
use crate::hw::{Device, GpuGraph};
use crate::route::Place;

pub struct CudaDevice {
    cuda: Device,
    base: Device,
    vulkan: Arc<Mutex<Option<std::result::Result<Device, String>>>>,
}

impl Clone for CudaDevice {
    fn clone(&self) -> Self {
        self.try_clone().unwrap_or_else(|e| panic!("{e}"))
    }
}

impl CudaDevice {
    pub fn open() -> Result<Self> {
        match crate::gpus::in_use().and_then(|g| g.cuda) {
            Some(index) => Self::on(index),
            None => Err(Error::Invalid("no NVIDIA GPU is in use".into())),
        }
    }

    pub(crate) fn on(index: i32) -> Result<Self> {
        static SHARED: Mutex<Vec<(i32, std::result::Result<CudaDevice, String>)>> =
            Mutex::new(Vec::new());
        let mut shared = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, opened)) = shared.iter().find(|(i, _)| *i == index) {
            return match opened {
                Ok(device) => device.try_clone(),
                Err(e) => Err(Error::Invalid(e.clone())),
            };
        }
        crate::init();
        let mut device = ptr::null_mut();
        let on = std::ffi::CString::new(index.to_string()).expect("digits");
        // SAFETY: FFmpeg makes the device on the CUDA device of that index.
        let ret = unsafe {
            ffi::av_hwdevice_ctx_create(
                &mut device,
                ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA,
                on.as_ptr(),
                ptr::null_mut(),
                0,
            )
        };
        if ret < 0 || device.is_null() {
            let err = format!("no CUDA device: {}", ffmpeg::Error::from(ret));
            shared.push((index, Err(err.clone())));
            return Err(Error::Invalid(err));
        }
        // SAFETY: the device FFmpeg just made, ours alone.
        let cuda = unsafe { Device::from_raw(device) };
        let base = cuda.clone()?;
        let opened = Self {
            base,
            cuda,
            vulkan: Arc::default(),
        };
        let stored = opened.try_clone()?;
        shared.push((index, Ok(stored)));
        Ok(opened)
    }

    pub(crate) fn try_clone(&self) -> Result<Self> {
        Ok(Self {
            cuda: self.cuda.clone()?,
            base: self.base.clone()?,
            vulkan: Arc::clone(&self.vulkan),
        })
    }

    pub fn with_own_stream(&self) -> Result<Self> {
        let stream = share::Stream::new(self)?;
        // SAFETY: a fresh device on the context `base` keeps, owning `stream` until its `free`.
        unsafe {
            let mut device = ffi::av_hwdevice_ctx_alloc(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA);
            if device.is_null() {
                return Err(Error::Invalid("no memory for a CUDA device".into()));
            }
            let hw = (*device).data as *mut ffi::AVHWDeviceContext;
            let cuda = (*hw).hwctx as *mut ffi::AVCUDADeviceContext;
            let own = (*((*self.as_ptr()).data as *const ffi::AVHWDeviceContext)).hwctx
                as *const ffi::AVCUDADeviceContext;
            (*cuda).cuda_ctx = (*own).cuda_ctx;
            (*cuda).stream = stream.as_raw().cast();
            (*hw).user_opaque = stream.into_raw();
            (*hw).free = Some(share::destroy_stream);
            let ret = ffi::av_hwdevice_ctx_init(device);
            if ret < 0 {
                ffi::av_buffer_unref(&mut device);
                return Err(Error::Ffmpeg {
                    context: "setting up a CUDA stream".into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
            Ok(Self {
                cuda: Device::from_raw(device),
                base: self.base.clone()?,
                vulkan: Arc::clone(&self.vulkan),
            })
        }
    }

    pub(crate) fn as_ptr(&self) -> *mut ffi::AVBufferRef {
        self.cuda.as_ptr()
    }

    pub(crate) fn vulkan(&self) -> Result<Device> {
        let mut slot = self.vulkan.lock().unwrap_or_else(|e| e.into_inner());
        match slot.get_or_insert_with(|| {
            let _one = crate::hw::making_vulkan();
            let mut vulkan = ptr::null_mut();
            // SAFETY: FFmpeg makes the device from this live one.
            let ret = unsafe {
                ffi::av_hwdevice_ctx_create_derived(
                    &mut vulkan,
                    ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VULKAN,
                    self.base.as_ptr(),
                    0,
                )
            };
            if ret < 0 || vulkan.is_null() {
                Err(format!(
                    "no Vulkan device on the CUDA device's GPU: {}",
                    ffmpeg::Error::from(ret)
                ))
            } else {
                // SAFETY: the device FFmpeg just derived, ours alone.
                Ok(unsafe { Device::from_raw(vulkan) })
            }
        }) {
            Ok(device) => device.clone(),
            Err(e) => Err(Error::Invalid(e.clone())),
        }
    }

    fn forget_vulkan(&self, used: &Device) {
        let mut slot = self.vulkan.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: `d` and `used` are live device references; the same `data` is the same device.
        let same = |d: &Device| unsafe { (*d.as_ptr()).data == (*used.as_ptr()).data };
        if let Some(Ok(d)) = slot.as_ref()
            && same(d)
        {
            *slot = None;
        }
    }
}

pub(crate) fn frame_sides(width: u32, height: u32) -> Result<(i32, i32)> {
    match (i32::try_from(width), i32::try_from(height)) {
        (Ok(width), Ok(height)) => Ok((width, height)),
        _ => Err(Error::Invalid(format!(
            "{width}×{height} doesn't fit a frame"
        ))),
    }
}

struct HwFrames(*mut ffi::AVBufferRef);

// SAFETY: as for the device.
unsafe impl Send for HwFrames {}

impl HwFrames {
    fn new(
        device: &Device,
        hw: ffi::AVPixelFormat,
        sw: ffi::AVPixelFormat,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let (width, height) = frame_sides(width, height)?;
        // SAFETY: a frames context FFmpeg allocates on this live device.
        unsafe {
            let mut frames = ffi::av_hwframe_ctx_alloc(device.as_ptr());
            if frames.is_null() {
                return Err(Error::Invalid("no memory for GPU frames".into()));
            }
            let fc = &mut *((*frames).data as *mut ffi::AVHWFramesContext);
            fc.format = hw;
            fc.sw_format = sw;
            fc.width = width;
            fc.height = height;
            let ret = ffi::av_hwframe_ctx_init(frames);
            if ret < 0 {
                ffi::av_buffer_unref(&mut frames);
                return Err(Error::Ffmpeg {
                    context: "setting up GPU frames".into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
            Ok(Self(frames))
        }
    }

    fn share(&self) -> Result<Self> {
        // SAFETY: a reference to this live frames context.
        let frames = unsafe { ffi::av_buffer_ref(self.0) };
        if frames.is_null() {
            return Err(Error::Invalid("no memory for GPU frames".into()));
        }
        Ok(Self(frames))
    }

    fn copy(&self, src: &frame::Video, what: &str) -> Result<frame::Video> {
        let mut hw = frame::Video::empty();
        // SAFETY: a fresh frame of this live pool, given `src`'s picture.
        unsafe {
            let ret = ffi::av_hwframe_get_buffer(self.0, hw.as_mut_ptr(), 0);
            if ret < 0 {
                return Err(Error::Ffmpeg {
                    context: "getting a GPU frame".into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
            let ret = ffi::av_hwframe_transfer_data(hw.as_mut_ptr(), src.as_ptr(), 0);
            if ret < 0 {
                return Err(Error::Ffmpeg {
                    context: what.into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
        }
        Ok(hw)
    }
}

impl Drop for HwFrames {
    fn drop(&mut self) {
        // SAFETY: this value's own reference.
        unsafe { ffi::av_buffer_unref(&mut self.0) };
    }
}

pub struct CudaFrames(HwFrames);

impl CudaFrames {
    pub fn new(device: &CudaDevice, sw: format::Pixel, width: u32, height: u32) -> Result<Self> {
        HwFrames::new(
            &device.cuda,
            ffi::AVPixelFormat::AV_PIX_FMT_CUDA,
            sw.into(),
            width,
            height,
        )
        .map(Self)
    }

    pub fn as_ptr(&self) -> *mut ffi::AVBufferRef {
        self.0.0
    }

    pub fn upload(&self, src: &frame::Video) -> Result<frame::Video> {
        let mut hw = self.0.copy(src, "uploading to the GPU")?;
        crate::copies::up(src);
        hw.set_pts(src.pts());
        Ok(hw)
    }
}

static SHADERS: OnceLock<PathBuf> = OnceLock::new();

pub fn keep_shaders_in(dir: PathBuf) {
    let _ = SHADERS.set(dir);
}

fn shader_cache() -> Option<String> {
    let dir = SHADERS.get()?;
    std::fs::create_dir_all(dir).ok()?;
    Some(format!("{}/", dir.to_str()?))
}

pub struct CudaPictures {
    takes: format::Pixel,
    graph: Option<GpuGraph>,
    placebo: Option<Placebo>,
    ready: VecDeque<frame::Video>,
}

struct Placebo {
    graph: GpuGraph,
    input: Option<HwFrames>,
    output: Option<HwFrames>,
    background: Option<Blacks>,
    filler: Option<Blacks>,
    tags: Tags,
    count: i64,
    pts: VecDeque<(i64, Option<i64>)>,
    sar: ffi::AVRational,
    device: CudaDevice,
    vulkan: Device,
    held: VecDeque<frame::Video>,
}

fn converting() -> std::sync::MutexGuard<'static, ()> {
    static CONVERTING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    CONVERTING.lock().unwrap_or_else(|e| e.into_inner())
}

impl Drop for Placebo {
    fn drop(&mut self) {
        let _one = converting();
        drop(std::mem::replace(&mut self.graph, GpuGraph::new()));
    }
}

const HANDOFF: std::time::Duration = std::time::Duration::from_secs(5);

fn wait_done(f: &frame::Video, what: &str) -> Result<()> {
    // SAFETY: `f` is a live Vulkan frame: on a Vulkan device, its `AVVkFrame` in `data[0]`.
    unsafe {
        let fc = (*(*f.as_ptr()).hw_frames_ctx).data as *const ffi::AVHWFramesContext;
        let dev = &*((*(*fc).device_ctx).hwctx as *const ffi::AVVulkanDeviceContext);
        wait_signalled(
            dev,
            &[(*f.as_ptr()).data[0] as *const ffi::AVVkFrame],
            HANDOFF,
        )
    }
    .map_err(|e| Error::Invalid(format!("{what}: {e}")))
}

pub(crate) unsafe fn wait_signalled(
    dev: &ffi::AVVulkanDeviceContext,
    frames: &[*const ffi::AVVkFrame],
    timeout: std::time::Duration,
) -> Result<()> {
    let (mut sems, mut values) = (Vec::new(), Vec::new());
    for &f in frames {
        // SAFETY: as the caller says.
        let f = unsafe { &*f };
        for (&sem, &value) in f.sem.iter().zip(&f.sem_value).take_while(|(s, _)| **s != 0) {
            sems.push(sem);
            values.push(value);
        }
    }
    if sems.is_empty() {
        return Ok(());
    }
    // SAFETY: the device's calls as vulkan.h declares them, looked up through its loader.
    unsafe {
        let none = |what: &str| Error::Invalid(format!("no {what}"));
        let instance_proc = dev.get_proc_addr.ok_or_else(|| none("Vulkan loader"))?;
        let device_proc = instance_proc(dev.inst, c"vkGetDeviceProcAddr".as_ptr())
            .ok_or_else(|| none("vkGetDeviceProcAddr"))?;
        let device_proc: GetDeviceProcAddr = std::mem::transmute(device_proc);
        let wait = device_proc(dev.act_dev, c"vkWaitSemaphores".as_ptr())
            .ok_or_else(|| none("vkWaitSemaphores"))?;
        let wait: WaitSemaphores = std::mem::transmute(wait);
        let info = SemaphoreWaitInfo {
            s_type: SEMAPHORE_WAIT_INFO,
            next: ptr::null(),
            flags: 0,
            count: sems.len() as u32,
            semaphores: sems.as_ptr(),
            values: values.as_ptr(),
        };
        match wait(dev.act_dev, &info, timeout.as_nanos() as u64) {
            0 => Ok(()),
            result => Err(Error::Invalid(waiting_failed(result, timeout))),
        }
    }
}

fn waiting_failed(result: i32, timeout: std::time::Duration) -> String {
    match result {
        VK_TIMEOUT => format!("the GPU didn't finish in {} s", timeout.as_secs()),
        VK_ERROR_DEVICE_LOST => "the GPU stopped working (its Vulkan device was lost)".into(),
        _ => format!("waiting for the GPU failed (Vulkan error {result})"),
    }
}

const VK_TIMEOUT: i32 = 2;

const VK_ERROR_DEVICE_LOST: i32 = -4;

type GetDeviceProcAddr =
    unsafe extern "system" fn(ffi::VkDevice, *const std::ffi::c_char) -> ffi::PFN_vkVoidFunction;

type WaitSemaphores =
    unsafe extern "system" fn(ffi::VkDevice, *const SemaphoreWaitInfo, u64) -> i32;

const SEMAPHORE_WAIT_INFO: i32 = 1_000_207_004;

#[repr(C)]
struct SemaphoreWaitInfo {
    s_type: i32,
    next: *const std::ffi::c_void,
    flags: u32,
    count: u32,
    semaphores: *const ffi::VkSemaphore,
    values: *const u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    Upright,
    Placed {
        size: (u32, u32),
        fitted: (u32, u32),
        format: format::Pixel,
    },
}

const COUNTED: ffi::AVRational = ffi::AVRational { num: 1, den: 1 };

fn filters(
    (w, h): (u32, u32),
    (fit_w, fit_h): (u32, u32),
    (width, height, out): (u32, u32, &str),
    fixed: &str,
) -> String {
    let options = format!(":format={out}");
    let mut steps = scaling_steps((w, h), (fit_w, fit_h), &options, fixed);
    if (fit_w, fit_h) != (width, height) {
        let (x, y) = centered((fit_w, fit_h), (width, height));
        steps.push(format!(
            "pad_cuda=w={width}:h={height}:x={x}:y={y}:color=black"
        ));
    }
    steps.join(",")
}

pub(crate) fn scaling(size: (u32, u32), fit: (u32, u32), options: &str) -> Vec<String> {
    scaling_steps(size, fit, options, fixed_kernels())
}

fn fixed_kernels() -> &'static str {
    static FIXED: OnceLock<&str> = OnceLock::new();
    FIXED.get_or_init(|| {
        if crate::parts::filter_has_option("scale_cuda", "use_filters") {
            ":use_filters=0"
        } else {
            ""
        }
    })
}

fn scaling_steps(
    (w, h): (u32, u32),
    (fit_w, fit_h): (u32, u32),
    options: &str,
    fixed: &str,
) -> Vec<String> {
    let mut steps = Vec::new();
    let (mut w, mut h) = (w, h);
    while w >= 2 * fit_w && h >= 2 * fit_h && w >= 4 && h >= 4 {
        (w, h) = ((w / 2) & !1, (h / 2) & !1);
        steps.push(format!(
            "scale_cuda=w={w}:h={h}:interp_algo=bilinear{fixed}"
        ));
    }
    steps.push(format!(
        "scale_cuda=w={fit_w}:h={fit_h}{options}:interp_algo=bicubic{fixed}"
    ));
    steps
}

fn centered((fit_w, fit_h): (u32, u32), (width, height): (u32, u32)) -> (u32, u32) {
    (((width - fit_w) / 2) & !1, ((height - fit_h) / 2) & !1)
}

fn placebo_options(
    (width, height): (u32, u32),
    rotation: u16,
    out: &str,
    layout: Layout,
    color: &GpuColor,
    up: Option<Sharpness>,
) -> Vec<(&'static str, String)> {
    let mut options: Vec<(&str, String)> = match layout {
        Layout::Upright if rotation % 180 == 90 => {
            vec![("w", height.to_string()), ("h", width.to_string())]
        }
        Layout::Upright => vec![("w", width.to_string()), ("h", height.to_string())],
        Layout::Placed {
            size: (w, h),
            fitted,
            ..
        } if fitted == (w, h) => vec![("w", w.to_string()), ("h", h.to_string())],
        Layout::Placed {
            size: (w, h),
            fitted: (fw, fh),
            ..
        } => {
            let (x, y) = centered((fw, fh), (w, h));
            vec![
                ("inputs", "2".into()),
                ("w", w.to_string()),
                ("h", h.to_string()),
                ("pos_x", format!("if(eq(idx,0),0,{x})")),
                ("pos_y", format!("if(eq(idx,0),0,{y})")),
                ("pos_w", format!("if(eq(idx,0),{w},{fw})")),
                ("pos_h", format!("if(eq(idx,0),{h},{fh})")),
            ]
        }
    };
    let name = |p: *const std::ffi::c_char| {
        if p.is_null() {
            String::new()
        } else {
            // SAFETY: one of FFmpeg's static names, NUL-terminated.
            unsafe { std::ffi::CStr::from_ptr(p) }
                .to_string_lossy()
                .into_owned()
        }
    };
    let o = color.output;
    // SAFETY: FFmpeg's static names of the tags, or null.
    let (matrix, primaries, transfer) = unsafe {
        (
            ffi::av_color_space_name(o.matrix),
            ffi::av_color_primaries_name(o.primaries),
            ffi::av_color_transfer_name(o.transfer),
        )
    };
    let upscaler = match up {
        None => "catmull_rom",
        Some(Sharpness::Low) => "ewa_lanczos",
        Some(Sharpness::Medium) => "ewa_lanczossharp",
        Some(Sharpness::High) => "ewa_lanczos4sharpest",
    };
    let linear = up.is_some() && color.shader.is_none();
    options.extend([
        ("rotate", rotation.to_string()),
        ("format", out.into()),
        ("colorspace", name(matrix)),
        ("color_primaries", name(primaries)),
        ("color_trc", name(transfer)),
        ("range", "tv".into()),
        ("upscaler", upscaler.into()),
        ("downscaler", "catmull_rom".into()),
        ("disable_linear", if linear { "0" } else { "1" }.into()),
        ("sigmoid", if linear { "1" } else { "0" }.into()),
        ("dithering", "none".into()),
        ("peak_detect", "0".into()),
        ("apply_filmgrain", "0".into()),
        ("apply_dolbyvision", "0".into()),
    ]);
    if let Some(shader) = &color.shader {
        let hex: String = shader.bytes().map(|b| format!("{b:02x}")).collect();
        options.push(("custom_shader_bin", hex));
    }
    if let Some(prefix) = shader_cache() {
        options.push(("shader_cache", prefix));
    }
    options
}

pub(crate) fn add_source(
    graph: &mut GpuGraph,
    name: &std::ffi::CStr,
    pixel: ffi::AVPixelFormat,
    (width, height): (u32, u32),
    (time_base, sar): (ffi::AVRational, ffmpeg::Rational),
    frames: *mut ffi::AVBufferRef,
    color: Option<(ffi::AVColorSpace, ffi::AVColorRange)>,
) -> Result<()> {
    let (width, height) = frame_sides(width, height)?;
    // SAFETY: a buffer source on `graph`; `frames` is a live frames context the caller holds.
    unsafe {
        let ctx = ffi::avfilter_graph_alloc_filter(
            graph.as_mut_ptr(),
            ffi::avfilter_get_by_name(c"buffer".as_ptr()),
            name.as_ptr(),
        );
        if ctx.is_null() {
            return Err(Error::Invalid("no memory for the GPU source".into()));
        }
        let par = ffi::av_buffersrc_parameters_alloc();
        if par.is_null() {
            return Err(Error::Invalid("no memory for the GPU source".into()));
        }
        (*par).format = pixel as i32;
        (*par).width = width;
        (*par).height = height;
        (*par).time_base = time_base;
        (*par).sample_aspect_ratio = ffi::AVRational {
            num: sar.numerator().max(1),
            den: sar.denominator().max(1),
        };
        (*par).hw_frames_ctx = frames;
        if let Some((matrix, range)) = color {
            (*par).color_space = matrix;
            (*par).color_range = range;
        }
        let set = ffi::av_buffersrc_parameters_set(ctx, par);
        ffi::av_free(par.cast());
        let ret = if set < 0 {
            set
        } else {
            ffi::avfilter_init_str(ctx, ptr::null())
        };
        if ret < 0 {
            return Err(Error::Ffmpeg {
                context: "creating the GPU source".into(),
                source: ffmpeg::Error::from(ret),
            });
        }
    }
    Ok(())
}

fn add_sink(graph: &mut GpuGraph) -> Result<()> {
    let sink = filter::find("buffersink").ok_or_else(|| Error::Invalid("no buffersink".into()))?;
    graph
        .add(&sink, "out", "")
        .ctx(|| "creating the GPU sink".into())?;
    Ok(())
}

fn take(graph: &mut GpuGraph, what: &str) -> Result<Option<(frame::Video, ffmpeg::Rational)>> {
    let mut sink = graph.get("out").expect("sink");
    let mut out = frame::Video::empty();
    match sink.sink().frame(&mut out) {
        Ok(()) => Ok(Some((out, sink.sink().time_base()))),
        Err(ffmpeg::Error::Eof)
        | Err(ffmpeg::Error::Other {
            errno: ffmpeg::util::error::EAGAIN,
        }) => Ok(None),
        Err(e) => Err(Error::Ffmpeg {
            context: what.into(),
            source: e,
        }),
    }
}

pub(crate) fn can_fit(ops: &crate::route::Ops) -> bool {
    // SAFETY: FFmpeg's static description of the format.
    let alpha = ops
        .layout
        .descriptor()
        .is_some_and(|d| unsafe { (*d.as_ptr()).flags & ffi::AV_PIX_FMT_FLAG_ALPHA as u64 != 0 });
    !ops.rgb && !alpha && matches!(ops.rotation, 0 | 90 | 180 | 270)
}

pub(crate) fn reaches(from: Place, to: Place) -> bool {
    matches!(
        (from, to),
        (Place::Cuda, Place::Vulkan) | (Place::Vulkan, Place::Cuda)
    )
}

impl CudaPictures {
    pub fn new(
        device: &CudaDevice,
        reader: &VideoReader,
        (width, height): (u32, u32),
        format: format::Pixel,
        target: Option<Hdr>,
        (at_once, on): (bool, Place),
        sharp: Option<Sharpness>,
    ) -> Result<Self> {
        let first = reader
            .peek()
            .ok_or_else(|| Error::Invalid("no frames".into()))?;
        let (time_base, rotation) = (reader.time_base(), reader.rotation());
        let vulkans = first.format() == format::Pixel::VULKAN;
        if !vulkans && first.format() != format::Pixel::CUDA {
            return Err(Error::Invalid("not frames in GPU memory".into()));
        }
        if !matches!(rotation, 0 | 90 | 180 | 270) {
            return Err(Error::Invalid(format!("a rotation of {rotation}°")));
        }
        if !matches!(on, Place::Cuda | Place::Vulkan) {
            return Err(Error::Invalid(format!("no pictures for {on:?}")));
        }
        let color = crate::color::gpu_color(first, target)
            .ok_or_else(|| Error::Invalid("RGB pictures".into()))?;
        let out = match format {
            format::Pixel::P010LE => "p010le",
            _ => "nv12",
        };
        let stored = (first.width(), first.height());
        let sar = first.aspect_ratio();
        let (size, shown_sar) = if rotation % 180 == 90 {
            (
                (stored.1, stored.0),
                ffmpeg::Rational(sar.denominator(), sar.numerator()),
            )
        } else {
            (stored, sar)
        };
        let fitted = crate::export::normalize::fit(
            size.0,
            size.1,
            (shown_sar.numerator(), shown_sar.denominator()),
            width,
            height,
        );
        // SAFETY: reads the frame's own field.
        let decoded = unsafe { (*first.as_ptr()).hw_frames_ctx };
        if decoded.is_null() {
            return Err(Error::Invalid(
                "decoded frames without a frames context".into(),
            ));
        }
        let placed = Layout::Placed {
            size: (width, height),
            fitted,
            format,
        };
        let up = sharp.filter(|_| {
            let sar = (sar.numerator(), sar.denominator());
            crate::export::normalize::enlarges(stored, sar, rotation, fitted)
        });
        let layout = if on == Place::Vulkan
            || vulkans
            || color.shader.is_some()
            || up.is_some()
            || (fitted != (width, height) && format == format::Pixel::P010LE)
        {
            Some(placed)
        } else if color.unchanged() && rotation == 0 {
            None
        } else {
            Some(Layout::Upright)
        };
        let placebo = match layout {
            Some(layout) => {
                let stream = (stored, sar, rotation);
                Some(Placebo::new(
                    device,
                    decoded,
                    stream,
                    &color,
                    layout,
                    (at_once, on),
                    up,
                )?)
            }
            None => None,
        };
        let ready = VecDeque::new();
        let takes = first.format();
        if on == Place::Vulkan {
            return Ok(Self {
                takes,
                graph: None,
                placebo,
                ready,
            });
        }
        let (size, spec, sar) = if let Some(Layout::Placed { .. }) = layout {
            ((width, height), "null".to_string(), ffmpeg::Rational(1, 1))
        } else {
            let spec = filters(size, fitted, (width, height, out), fixed_kernels());
            (size, spec, shown_sar)
        };
        let frames = match placebo.as_ref().and_then(|p| p.output.as_ref()) {
            Some(output) => output.0,
            None => decoded,
        };
        let mut graph = GpuGraph::new();
        add_source(
            &mut graph,
            c"in",
            ffi::AVPixelFormat::AV_PIX_FMT_CUDA,
            size,
            (time_base.into(), sar),
            frames,
            None,
        )?;
        add_sink(&mut graph)?;
        graph.parse_filters(&spec)?;
        graph.validate().ctx(|| format!("configuring {spec}"))?;
        Ok(Self {
            takes,
            graph: Some(graph),
            placebo,
            ready,
        })
    }

    pub(crate) fn vulkan(&self) -> Option<&Device> {
        self.placebo.as_ref().map(|p| &p.vulkan)
    }

    pub fn push(&mut self, f: Option<&frame::Video>) -> Result<()> {
        if let Some(f) = f
            && f.format() != self.takes
        {
            return Err(Error::Invalid(format!(
                "the GPU's scaler takes {:?} frames, not {:?} ones",
                self.takes,
                f.format()
            )));
        }
        let Some(p) = &mut self.placebo else {
            let mut source = self
                .graph
                .as_mut()
                .expect("filters")
                .get("in")
                .expect("source");
            return match f {
                Some(f) => source.source().add(f).ctx(|| "scaling on the GPU".into()),
                None => source
                    .source()
                    .flush()
                    .ctx(|| "flushing the GPU scaler".into()),
            };
        };
        p.push(f)?;
        while let Some(converted) = p.pull()? {
            match &mut self.graph {
                Some(graph) => graph
                    .get("in")
                    .expect("source")
                    .source()
                    .add(&converted)
                    .ctx(|| "scaling on the GPU".into())?,
                None => self.ready.push_back(converted),
            }
        }
        if f.is_none()
            && let Some(graph) = &mut self.graph
        {
            graph
                .get("in")
                .expect("source")
                .source()
                .flush()
                .ctx(|| "flushing the GPU scaler".into())?;
        }
        Ok(())
    }

    pub fn pull(&mut self) -> Result<Option<frame::Video>> {
        let Some(graph) = &mut self.graph else {
            return Ok(self.ready.pop_front());
        };
        let Some((mut out, _)) = take(graph, "scaling on the GPU")? else {
            return Ok(None);
        };
        // SAFETY: a frame this function owns.
        unsafe {
            (*out.as_mut_ptr()).sample_aspect_ratio = ffi::AVRational { num: 1, den: 1 };
        }
        Ok(Some(out))
    }
}

impl Placebo {
    fn new(
        device: &CudaDevice,
        decoded: *mut ffi::AVBufferRef,
        ((width, height), sar, rotation): ((u32, u32), ffmpeg::Rational, u16),
        color: &GpuColor,
        layout: Layout,
        (at_once, on): (bool, Place),
        up: Option<Sharpness>,
    ) -> Result<Self> {
        use ffi::AVPixelFormat::{AV_PIX_FMT_CUDA as CUDA, AV_PIX_FMT_VULKAN as VULKAN};
        let _one = converting();
        // SAFETY: a live frames context.
        let (sw, vulkans, frames_device) = unsafe {
            let fc = &*((*decoded).data as *const ffi::AVHWFramesContext);
            (fc.sw_format, fc.format == VULKAN, fc.device_ref)
        };
        let vulkan = if vulkans {
            // SAFETY: the frames' own device, another reference to it.
            let own = unsafe { ffi::av_buffer_ref(frames_device) };
            if own.is_null() {
                return Err(Error::Invalid("no memory for a Vulkan device".into()));
            }
            // SAFETY: the reference just taken, ours alone.
            unsafe { Device::from_raw(own) }
        } else {
            device.vulkan()?
        };
        let turned = rotation % 180 == 90;
        let (out_size, out_pixel, out_sar) = match layout {
            Layout::Upright => {
                // SAFETY: FFmpeg's static description of the format.
                let deep = format::Pixel::from(sw)
                    .descriptor()
                    .is_some_and(|d| unsafe { (*d.as_ptr()).comp[0].depth } > 8);
                let pixel = if deep {
                    format::Pixel::P010LE
                } else {
                    format::Pixel::NV12
                };
                let (n, d) = (sar.numerator().max(1), sar.denominator().max(1));
                if turned {
                    ((height, width), pixel, (d, n))
                } else {
                    ((width, height), pixel, (n, d))
                }
            }
            Layout::Placed { size, format, .. } => (size, format, (1, 1)),
        };
        let (out, vulkan_out) = match out_pixel {
            format::Pixel::P010LE => ("p016le", format::Pixel::P016LE),
            format::Pixel::YUV422P10LE => ("yuv422p10le", format::Pixel::YUV422P10LE),
            _ => ("nv12", format::Pixel::NV12),
        };
        let pool = HwFrames::new(&vulkan, VULKAN, as_read(sw), width, height)?;
        let output = match on {
            Place::Vulkan => None,
            _ => Some(HwFrames::new(
                &device.cuda,
                CUDA,
                out_pixel.into(),
                out_size.0,
                out_size.1,
            )?),
        };
        let mut graph = GpuGraph::new();
        let black_tags = color.output;
        let background = match layout {
            Layout::Placed {
                size: (w, h),
                fitted,
                ..
            } if fitted != (w, h) => {
                let format = vulkan_out;
                let frames = HwFrames::new(&vulkan, VULKAN, format.into(), w, h)?;
                let timing = (COUNTED, ffmpeg::Rational(1, 1));
                let tags = Some((black_tags.matrix, black_tags.range));
                add_source(&mut graph, c"bg", VULKAN, (w, h), timing, frames.0, tags)?;
                Some(Blacks::new(frames, format, (w, h), black_tags)?)
            }
            _ => None,
        };
        let timing = (COUNTED, sar);
        let tags = color.input;
        let fed = if vulkans { decoded } else { pool.0 };
        add_source(
            &mut graph,
            c"in",
            VULKAN,
            (width, height),
            timing,
            fed,
            Some((tags.matrix, tags.range)),
        )?;
        add_sink(&mut graph)?;
        let options = placebo_options((width, height), rotation, out, layout, color, up);
        // SAFETY: `ctx` lives in `graph` with the filters it links to; `vulkan` is a live device.
        unsafe {
            let ctx = ffi::avfilter_graph_alloc_filter(
                graph.as_mut_ptr(),
                ffi::avfilter_get_by_name(c"libplacebo".as_ptr()),
                c"placebo".as_ptr(),
            );
            if ctx.is_null() {
                return Err(Error::Invalid("no libplacebo filter".into()));
            }
            (*ctx).hw_device_ctx = ffi::av_buffer_ref(vulkan.as_ptr());
            if (*ctx).hw_device_ctx.is_null() {
                return Err(Error::Invalid("no memory for libplacebo's device".into()));
            }
            let mut dict = ffmpeg::Dictionary::new();
            for (k, v) in &options {
                dict.set(k, v);
            }
            let mut raw = dict.disown();
            let ret = ffi::avfilter_init_dict(ctx, &mut raw);
            ffi::av_dict_free(&mut raw);
            if ret < 0 {
                return Err(Error::Ffmpeg {
                    context: "starting libplacebo".into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
            let mut sources = Vec::new();
            if background.is_some() {
                sources.push(graph.get("bg").expect("background").as_mut_ptr());
            }
            sources.push(graph.get("in").expect("source").as_mut_ptr());
            let sink = graph.get("out").expect("sink").as_mut_ptr();
            let mut ret = 0;
            for (pad, src) in sources.into_iter().enumerate() {
                if ret >= 0 {
                    ret = ffi::avfilter_link(src, 0, ctx, pad as u32);
                }
            }
            if ret >= 0 {
                ret = ffi::avfilter_link(ctx, 0, sink, 0);
            }
            if ret < 0 {
                return Err(Error::Ffmpeg {
                    context: "linking libplacebo".into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
        }
        graph.validate().ctx(|| "configuring libplacebo".into())?;
        let filler = if at_once {
            let size = (width, height);
            Some(Blacks::new(
                pool.share()?,
                as_read(sw).into(),
                size,
                color.input,
            )?)
        } else {
            None
        };
        Ok(Self {
            graph,
            input: (!vulkans).then_some(pool),
            output,
            background,
            filler,
            tags: color.input,
            count: 0,
            pts: VecDeque::new(),
            sar: ffi::AVRational {
                num: out_sar.0,
                den: out_sar.1,
            },
            device: device.try_clone()?,
            vulkan,
            held: VecDeque::new(),
        })
    }

    fn checked<T>(&self, r: Result<T>) -> Result<T> {
        if r.is_err() {
            self.device.forget_vulkan(&self.vulkan);
        }
        r
    }

    fn push(&mut self, f: Option<&frame::Video>) -> Result<()> {
        let _one = converting();
        let fed = self.feed_frame(f);
        self.checked(fed)
    }

    fn feed_frame(&mut self, f: Option<&frame::Video>) -> Result<()> {
        let Some(f) = f else {
            for name in ["bg", "in"] {
                if let Some(mut source) = self.graph.get(name) {
                    source
                        .source()
                        .flush()
                        .ctx(|| "flushing libplacebo".into())?;
                }
            }
            return Ok(());
        };
        let mut v = match &self.input {
            Some(input) => {
                let v = input.copy(f, "copying to Vulkan")?;
                wait_done(&v, "copying to Vulkan")?;
                v
            }
            None => {
                wait_done(f, "decoding on Vulkan")?;
                self.held.push_back(crate::export::video::frame_ref(f)?);
                crate::export::video::frame_ref(f)?
            }
        };
        copy_props(&mut v, f)?;
        // SAFETY: a frame this function owns.
        unsafe {
            let raw = &mut *v.as_mut_ptr();
            ffi::av_frame_side_data_free(&mut raw.side_data, &mut raw.nb_side_data);
        }
        tag(&mut v, self.tags);
        let n = 2 * self.count;
        self.count += 1;
        self.pts.push_back((n, f.pts()));
        self.feed(n, v)?;
        if let Some(filler) = self.filler.as_mut() {
            let next = filler.next()?;
            self.feed(n + 1, next)?;
        }
        Ok(())
    }

    fn feed(&mut self, n: i64, mut picture: frame::Video) -> Result<()> {
        if let Some(background) = self.background.as_mut() {
            let mut black = background.next()?;
            black.set_pts(Some(n));
            let mut source = self.graph.get("bg").expect("background");
            source
                .source()
                .add(&black)
                .ctx(|| "converting on the GPU".into())?;
        }
        picture.set_pts(Some(n));
        let mut source = self.graph.get("in").expect("source");
        source
            .source()
            .add(&picture)
            .ctx(|| "converting on the GPU".into())
    }

    fn pull(&mut self) -> Result<Option<frame::Video>> {
        let _one = converting();
        let pulled = self.converted();
        self.checked(pulled)
    }

    fn converted(&mut self) -> Result<Option<frame::Video>> {
        while let Some((out, tb)) = take(&mut self.graph, "converting on the GPU")? {
            // SAFETY: arithmetic on values.
            let k = out
                .pts()
                .map_or(-1, |k| unsafe { ffi::av_rescale_q(k, tb.into(), COUNTED) });
            if k % 2 != 0 {
                continue;
            }
            let mut pts = None;
            while let Some(&(j, t)) = self.pts.front()
                && j <= k
            {
                self.pts.pop_front();
                pts = t;
            }
            wait_done(&out, "converting on the GPU")?;
            self.held.clear();
            let mut c = match &self.output {
                Some(output) => {
                    let mut c = output.copy(&out, "copying back from Vulkan")?;
                    copy_props(&mut c, &out)?;
                    c
                }
                None => out,
            };
            // SAFETY: a frame this function owns.
            unsafe { (*c.as_mut_ptr()).sample_aspect_ratio = self.sar };
            c.set_pts(pts);
            return Ok(Some(c));
        }
        Ok(None)
    }
}

struct Blacks {
    pool: HwFrames,
    black: frame::Video,
    tags: Tags,
    made: Vec<frame::Video>,
}

impl Blacks {
    fn new(pool: HwFrames, format: format::Pixel, (w, h): (u32, u32), tags: Tags) -> Result<Self> {
        let mut black = frame::Video::new(format, w, h);
        crate::export::video::fill_black(&mut black)?;
        Ok(Self {
            pool,
            black,
            tags,
            made: Vec::new(),
        })
    }

    fn next(&mut self) -> Result<frame::Video> {
        let free = self.made.iter().position(|f| {
            // SAFETY: a frame of a pool holds its picture in `buf[0]`.
            unsafe { ffi::av_buffer_get_ref_count((*f.as_ptr()).buf[0]) == 1 }
        });
        let i = match free {
            Some(i) => i,
            None => {
                let mut f = self.pool.copy(&self.black, "copying to Vulkan")?;
                tag(&mut f, self.tags);
                self.made.push(f);
                self.made.len() - 1
            }
        };
        crate::export::video::frame_ref(&self.made[i])
    }
}

fn as_read(sw: ffi::AVPixelFormat) -> ffi::AVPixelFormat {
    match sw {
        ffi::AVPixelFormat::AV_PIX_FMT_P010LE => ffi::AVPixelFormat::AV_PIX_FMT_P016LE,
        other => other,
    }
}

fn copy_props(dst: &mut frame::Video, src: &frame::Video) -> Result<()> {
    // SAFETY: two live frames.
    let ret = unsafe { ffi::av_frame_copy_props(dst.as_mut_ptr(), src.as_ptr()) };
    if ret < 0 {
        return Err(Error::Ffmpeg {
            context: "copying a frame's properties".into(),
            source: ffmpeg::Error::from(ret),
        });
    }
    Ok(())
}

fn tag(f: &mut frame::Video, tags: Tags) {
    // SAFETY: a frame the caller owns.
    unsafe {
        let f = &mut *f.as_mut_ptr();
        f.colorspace = tags.matrix;
        f.color_range = tags.range;
        f.color_primaries = tags.primaries;
        f.color_trc = tags.transfer;
        f.duration = 1;
    }
}

#[cfg(test)]
mod tests {
    use super::{Layout, can_fit, filters, placebo_options};
    use crate::color::{GpuColor, gpu_color};
    use crate::ffmpeg::{self, ffi, format::Pixel, frame};
    use crate::route::Ops;

    fn sd() -> GpuColor {
        gpu_color(&frame::Video::new(Pixel::YUV420P, 720, 480), None).unwrap()
    }

    fn upright_yuv() -> Ops {
        Ops {
            rotation: 0,
            layout: Pixel::NV12,
            out_layout: Pixel::NV12,
            rgb: false,
            colors_kept: true,
            pads: false,
            into: false,
        }
    }

    #[test]
    fn upright_yuv_frames_fit_on_the_gpu() {
        assert!(can_fit(&upright_yuv()));
    }

    #[test]
    fn rgb_pictures_dont_fit_on_the_gpu() {
        assert!(!can_fit(&Ops {
            rgb: true,
            ..upright_yuv()
        }));
    }

    #[test]
    fn pictures_with_alpha_dont_fit_on_the_gpu() {
        assert!(!can_fit(&Ops {
            layout: Pixel::YUVA444P12LE,
            ..upright_yuv()
        }));
    }

    #[test]
    fn a_turn_other_than_quarter_turns_doesnt_fit_on_the_gpu() {
        assert!(!can_fit(&Ops {
            rotation: 45,
            ..upright_yuv()
        }));
    }

    fn option<'a>(options: &'a [(&str, String)], key: &str) -> Option<&'a str> {
        options
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn big_shrinks_halve_first() {
        assert_eq!(
            filters((3840, 2160), (960, 540), (960, 540, "nv12"), ""),
            "scale_cuda=w=1920:h=1080:interp_algo=bilinear,\
             scale_cuda=w=960:h=540:interp_algo=bilinear,\
             scale_cuda=w=960:h=540:format=nv12:interp_algo=bicubic"
        );
        assert_eq!(
            filters((1280, 720), (960, 540), (960, 540, "nv12"), ""),
            "scale_cuda=w=960:h=540:format=nv12:interp_algo=bicubic"
        );
        assert_eq!(
            filters((1920, 800), (1920, 800), (1920, 1080, "nv12"), ""),
            "scale_cuda=w=1920:h=800:format=nv12:interp_algo=bicubic,\
             pad_cuda=w=1920:h=1080:x=0:y=140:color=black"
        );
        assert_eq!(
            filters((1280, 534), (1280, 534), (1280, 720, "nv12"), ""),
            "scale_cuda=w=1280:h=534:format=nv12:interp_algo=bicubic,\
             pad_cuda=w=1280:h=720:x=0:y=92:color=black"
        );
    }

    #[test]
    fn every_step_asks_for_the_fixed_kernels_where_scale_cuda_has_filters() {
        assert_eq!(
            filters(
                (1920, 1080),
                (960, 540),
                (960, 540, "nv12"),
                ":use_filters=0"
            ),
            "scale_cuda=w=960:h=540:interp_algo=bilinear:use_filters=0,\
             scale_cuda=w=960:h=540:format=nv12:interp_algo=bicubic:use_filters=0"
        );
    }

    #[test]
    fn libplacebo_turns_pictures_upright() {
        let options = placebo_options((1920, 1080), 90, "nv12", Layout::Upright, &sd(), None);
        let get = |k| option(&options, k);
        assert_eq!((get("w"), get("h")), (Some("1080"), Some("1920")));
        assert_eq!(get("rotate"), Some("90"));
        let options = placebo_options((1920, 1080), 180, "nv12", Layout::Upright, &sd(), None);
        let get = |k| option(&options, k);
        assert_eq!((get("w"), get("h")), (Some("1920"), Some("1080")));
    }

    #[test]
    fn a_frame_wider_than_i32_doesnt_fit() {
        let err = super::frame_sides(2_147_483_648, 1080).unwrap_err();
        assert_eq!(err.to_string(), "2147483648×1080 doesn't fit a frame");
    }

    #[test]
    fn a_frame_taller_than_i32_doesnt_fit() {
        let err = super::frame_sides(1920, 2_147_483_648).unwrap_err();
        assert_eq!(err.to_string(), "1920×2147483648 doesn't fit a frame");
    }

    #[test]
    fn the_largest_i32_frame_fits() {
        assert_eq!(
            super::frame_sides(2_147_483_647, 2_147_483_647).unwrap(),
            (2_147_483_647, 2_147_483_647)
        );
    }

    #[test]
    fn gpu_frames_reject_a_side_past_i32() {
        // SAFETY: null, and only dropped: the side is rejected before the device is read.
        let none = unsafe { super::Device::from_raw(std::ptr::null_mut()) };
        let err = match super::HwFrames::new(
            &none,
            ffi::AVPixelFormat::AV_PIX_FMT_NONE,
            ffi::AVPixelFormat::AV_PIX_FMT_NV12,
            2_147_483_648,
            1,
        ) {
            Err(err) => err,
            Ok(_) => panic!("a side past i32 was accepted"),
        };
        assert_eq!(err.to_string(), "2147483648×1 doesn't fit a frame");
    }

    #[test]
    fn a_gpu_source_rejects_a_side_past_i32() {
        crate::init();
        let mut graph = super::GpuGraph::new();
        let err = super::add_source(
            &mut graph,
            c"in",
            ffi::AVPixelFormat::AV_PIX_FMT_CUDA,
            (1920, 2_147_483_648),
            (
                ffi::AVRational { num: 1, den: 1 },
                ffmpeg::Rational::new(1, 1),
            ),
            std::ptr::null_mut(),
            None,
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "1920×2147483648 doesn't fit a frame");
    }

    #[test]
    fn libplacebo_places_pictures_on_black() {
        let placed = Layout::Placed {
            size: (1920, 1080),
            fitted: (1920, 800),
            format: Pixel::P010LE,
        };
        let options = placebo_options((3840, 1600), 0, "p010le", placed, &sd(), None);
        let get = |k| option(&options, k);
        assert_eq!(get("inputs"), Some("2"));
        assert_eq!((get("w"), get("h")), (Some("1920"), Some("1080")));
        assert_eq!(get("pos_x"), Some("if(eq(idx,0),0,0)"));
        assert_eq!(get("pos_y"), Some("if(eq(idx,0),0,140)"));
        assert_eq!(get("pos_w"), Some("if(eq(idx,0),1920,1920)"));
        assert_eq!(get("pos_h"), Some("if(eq(idx,0),1080,800)"));
        let filled = Layout::Placed {
            size: (1920, 1080),
            fitted: (1920, 1080),
            format: Pixel::NV12,
        };
        let options = placebo_options((3840, 2160), 0, "nv12", filled, &sd(), None);
        let get = |k| option(&options, k);
        assert_eq!(get("inputs"), None);
        assert_eq!((get("w"), get("h")), (Some("1920"), Some("1080")));
        assert_eq!(get("pos_w"), None);
    }
}
