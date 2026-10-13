//! The NVIDIA GPU as one pipeline: frames NVDEC decodes stay in GPU memory,
//! are deinterlaced there by the reader (`bwdif_cuda`), scaled (`scale_cuda`,
//! then `pad_cuda`) and go to NVENC as they are. Pictures stored
//! turned (spec D33), in another matrix than BT.709 (BT.601 SD, VP8), in
//! full range, or in HDR or for HDR output (spec D35) are turned upright and
//! converted first by libplacebo on Vulkan, on the same GPU. Proxies
//! (decision D41) and exports use it: the CPU only reads the file and writes
//! the result.

pub mod share;

/// One plane's place in memory a display shares (`share::SharedBuffer`):
/// where it starts, how far apart its rows are, and how many bytes of how
/// many rows it has.
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
use std::sync::{Arc, OnceLock};

use vidcut_core::model::Hdr;

use crate::color::{GpuColor, Tags};
use crate::decode::VideoReader;
use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, ffi, filter, format, frame};
use crate::hw::Device;

/// A CUDA device: NVDEC, the CUDA filters and NVENC share it.
#[derive(Clone)]
pub struct CudaDevice {
    cuda: Device,
    /// The process's one device ([`CudaDevice::open`]): this one, or the
    /// one whose context this one shares with a stream of its own
    /// ([`CudaDevice::with_own_stream`]).
    base: Device,
    /// The Vulkan device on the same GPU, for libplacebo: made from the
    /// base the first time it is needed (that takes a quarter of a
    /// second), then shared by every clone.
    vulkan: Arc<OnceLock<std::result::Result<Device, String>>>,
}

impl CudaDevice {
    /// The CUDA device on the first GPU that works: one for the whole
    /// process, shared by every export and proxy, and never closed. Its
    /// Vulkan device is made once too: the Vulkan loader crashed when two
    /// threads made instances at the same time (two exports at once).
    pub fn open() -> Result<Self> {
        static SHARED: OnceLock<std::result::Result<CudaDevice, String>> = OnceLock::new();
        SHARED
            .get_or_init(|| {
                crate::init();
                let mut device = ptr::null_mut();
                let ret = unsafe {
                    ffi::av_hwdevice_ctx_create(
                        &mut device,
                        ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA,
                        ptr::null(),
                        ptr::null_mut(),
                        0,
                    )
                };
                if ret < 0 || device.is_null() {
                    return Err(format!("no CUDA device: {}", ffmpeg::Error::from(ret)));
                }
                let cuda = Device(device);
                Ok(Self {
                    base: cuda.clone(),
                    cuda,
                    vulkan: Arc::default(),
                })
            })
            .clone()
            .map_err(Error::Invalid)
    }

    /// A device of its own on the same context: its work goes on a stream of
    /// its own, so it never waits behind another user's. On the context's
    /// default stream every user's work runs in one line, and a wait there
    /// for Vulkan (libplacebo's pictures) holds up everyone's decoding and
    /// encoding; two users each waiting for Vulkan work queued behind the
    /// other's would never go on. Frames made on it are done once its
    /// stream is (`share::wait_for`).
    pub fn with_own_stream(&self) -> Result<Self> {
        let stream = share::Stream::new(self)?;
        // SAFETY: a CUDA device context FFmpeg allocates, given this
        // device's context (which the base keeps) and the new stream, which
        // its `free` destroys once FFmpeg is done with it.
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
                cuda: Device(device),
                base: self.base.clone(),
                vulkan: self.vulkan.clone(),
            })
        }
    }

    pub(crate) fn as_ptr(&self) -> *mut ffi::AVBufferRef {
        self.cuda.0
    }

    fn vulkan(&self) -> Result<&Device> {
        self.vulkan
            .get_or_init(|| {
                let mut vulkan = ptr::null_mut();
                let ret = unsafe {
                    ffi::av_hwdevice_ctx_create_derived(
                        &mut vulkan,
                        ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VULKAN,
                        self.base.0,
                        0,
                    )
                };
                if ret < 0 || vulkan.is_null() {
                    Err(format!(
                        "no Vulkan device on the CUDA device's GPU: {}",
                        ffmpeg::Error::from(ret)
                    ))
                } else {
                    Ok(Device(vulkan))
                }
            })
            .as_ref()
            .map_err(|e| Error::Invalid(e.clone()))
    }
}

/// Frames in GPU memory of one size and format.
struct HwFrames(*mut ffi::AVBufferRef);

// SAFETY: as for the device.
unsafe impl Send for HwFrames {}

impl HwFrames {
    /// `width`×`height` frames of `sw` laid out as `hw` (CUDA or Vulkan)
    /// on `device`.
    fn new(
        device: *mut ffi::AVBufferRef,
        hw: ffi::AVPixelFormat,
        sw: ffi::AVPixelFormat,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        unsafe {
            let mut frames = ffi::av_hwframe_ctx_alloc(device);
            if frames.is_null() {
                return Err(Error::Invalid("no memory for GPU frames".into()));
            }
            let fc = &mut *((*frames).data as *mut ffi::AVHWFramesContext);
            fc.format = hw;
            fc.sw_format = sw;
            fc.width = width as i32;
            fc.height = height as i32;
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

    /// Another reference to the same frames.
    fn share(&self) -> Self {
        Self(unsafe { ffi::av_buffer_ref(self.0) })
    }

    /// A frame of this pool holding the picture of `src` (in memory, or in
    /// GPU memory of another kind), no other properties.
    fn copy(&self, src: &frame::Video, what: &str) -> Result<frame::Video> {
        let mut hw = frame::Video::empty();
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
        unsafe { ffi::av_buffer_unref(&mut self.0) };
    }
}

/// GPU frames of one size and format: what NVENC is opened with, and where
/// pictures made on the CPU go up to the GPU.
pub struct CudaFrames(HwFrames);

impl CudaFrames {
    /// `width`×`height` frames of `sw` (NV12 or P010) on `device`.
    pub fn new(device: &CudaDevice, sw: format::Pixel, width: u32, height: u32) -> Result<Self> {
        HwFrames::new(
            device.as_ptr(),
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

    /// `src` (in memory, of this pool's format and size) copied into a GPU
    /// frame, keeping its timestamp.
    pub fn upload(&self, src: &frame::Video) -> Result<frame::Video> {
        let mut hw = self.0.copy(src, "uploading to the GPU")?;
        hw.set_pts(src.pts());
        Ok(hw)
    }
}

/// Where libplacebo keeps the shaders it compiles (`keep_shaders_in`).
static SHADERS: OnceLock<PathBuf> = OnceLock::new();

/// Keeps libplacebo's compiled shaders in `dir`: compiling them takes a
/// tenth of a second for each piece of an export it converts, reading them
/// back a fifth of that. Without it they are compiled every time.
pub fn keep_shaders_in(dir: PathBuf) {
    let _ = SHADERS.set(dir);
}

/// The prefix libplacebo names its shader files with, once the folder is
/// there.
fn shader_cache() -> Option<String> {
    let dir = SHADERS.get()?;
    std::fs::create_dir_all(dir).ok()?;
    Some(format!("{}/", dir.to_str()?))
}

/// Pictures for NVENC from frames NVDEC decoded (and the reader
/// deinterlaced, one frame per frame, as the CPU reader does): scaled to
/// fit `width`×`height` keeping the display aspect and padded with black to
/// it, as the CPU's normalizer does, as `format` (8-bit NV12 or 10-bit
/// P010) in the output's colors, square pixels, all in GPU memory.
/// Timestamps stay the source's.
pub struct CudaPictures {
    graph: filter::Graph,
    placebo: Option<Placebo>,
}

/// Pictures turned upright and in the output's colors, by libplacebo on
/// Vulkan (the CUDA filters neither turn pictures nor change their colors):
/// each frame is copied to a Vulkan frame, converted and copied back to a
/// CUDA frame, never leaving the GPU.
struct Placebo {
    graph: filter::Graph,
    input: HwFrames,
    output: HwFrames,
    /// Black, under each picture placed on a bigger frame
    /// (`Layout::Placed`).
    background: Option<Blacks>,
    /// Pushed after each frame when its picture is wanted at once.
    /// libplacebo gives out a frame's picture only once it has the frame
    /// after it; the filler's own pictures are dropped. Without it, a
    /// picture comes a frame late and the GPU renders half as many.
    filler: Option<Blacks>,
    /// What libplacebo reads the source as (`GpuColor::input`).
    tags: Tags,
    /// Frames are timed by count, two apart with the filler between, as
    /// libplacebo wants rising timestamps; their own wait here.
    count: i64,
    pts: VecDeque<(i64, Option<i64>)>,
    /// The sample aspect of the upright pictures.
    sar: ffi::AVRational,
}

/// One converter at work at a time in the process: made, fed, read or
/// torn down. Two at work at once lost the Vulkan device now and then as
/// they started (the three parallel `nvenc_exports` tests, one run in 76).
fn converting() -> std::sync::MutexGuard<'static, ()> {
    static CONVERTING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    CONVERTING.lock().unwrap_or_else(|e| e.into_inner())
}

impl Drop for Placebo {
    fn drop(&mut self) {
        let _one = converting();
        drop(std::mem::replace(&mut self.graph, filter::Graph::new()));
    }
}

/// How long the GPU may take with a frame before it counts as stuck.
const HANDOFF: std::time::Duration = std::time::Duration::from_secs(5);

/// Waits, [`HANDOFF`] at most, until the GPU has done what `f` (a Vulkan
/// frame) was last given: its copy from CUDA, or libplacebo's picture.
///
/// libplacebo without its commit 2ba21c07 gives a frame back (signals its
/// semaphore one step on) without waiting for its own read of the frame
/// when that read runs on another queue, as its compute passes do. Its
/// signal can then come before the copy's: the read starts on it and sees
/// the frame's old picture, and the copy's later signal sets the semaphore
/// back a step, so freeing the frame waits forever. And the frame, given
/// back, can be copied into again while libplacebo still reads it. So a
/// frame goes to libplacebo only once its copy is done, and is copied into
/// again only once libplacebo's picture of it is done.
/// docs/todo/libplacebo-handback-workaround.md says when this can go.
fn wait_done(f: &frame::Video, what: &str) -> Result<()> {
    // SAFETY: a Vulkan frame: its frames context's device, its AVVkFrame in
    // data[0].
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

/// Waits, `timeout` at most, until `dev`'s GPU has given its last signal on
/// each of `frames`, as their semaphores' values say.
///
/// # Safety
/// The frames are `dev`'s, alive.
unsafe fn wait_signalled(
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
    // SAFETY: the device's calls as vulkan.h declares them, looked up
    // through its loader.
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
            result => Err(Error::Invalid(format!(
                "the GPU didn't finish in {} s (VkResult {result})",
                timeout.as_secs()
            ))),
        }
    }
}

/// `vkGetDeviceProcAddr`.
type GetDeviceProcAddr =
    unsafe extern "system" fn(ffi::VkDevice, *const std::ffi::c_char) -> ffi::PFN_vkVoidFunction;

/// `vkWaitSemaphores`.
type WaitSemaphores =
    unsafe extern "system" fn(ffi::VkDevice, *const SemaphoreWaitInfo, u64) -> i32;

/// `VK_STRUCTURE_TYPE_SEMAPHORE_WAIT_INFO`.
const SEMAPHORE_WAIT_INFO: i32 = 1_000_207_004;

/// `VkSemaphoreWaitInfo`.
#[repr(C)]
struct SemaphoreWaitInfo {
    s_type: i32,
    next: *const std::ffi::c_void,
    flags: u32,
    count: u32,
    semaphores: *const ffi::VkSemaphore,
    values: *const u64,
}

/// What libplacebo makes of a picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// Upright at its own size, for the CUDA filters to scale and pad.
    Upright,
    /// Fitted to `fitted` in the middle of `size`, as `format`: pictures
    /// whose transfer it converts (HDR), as the CPU converts them after
    /// scaling, and the padding FFmpeg's `pad_cuda` can't do (10 bits).
    /// libplacebo can't clear a border in FFmpeg's Vulkan frames (it clears
    /// with a copy they don't allow), so when the picture leaves a border, a
    /// black frame of the size goes in under it and leaves none to clear.
    Placed {
        size: (u32, u32),
        fitted: (u32, u32),
        format: format::Pixel,
    },
}

/// The time base of the counted timestamps libplacebo sees: a second
/// apart, as libplacebo takes a frame within a millisecond of the time it
/// renders for as that time's frame, and may blend neighbors when pictures
/// come under a tenth of a second apart.
const COUNTED: ffi::AVRational = ffi::AVRational { num: 1, den: 1 };

/// The filters from `w`×`h` frames to `width`×`height` NV12 pictures.
fn filters(
    (w, h): (u32, u32),
    (fit_w, fit_h): (u32, u32),
    (width, height, out): (u32, u32, &str),
) -> String {
    let mut steps = scaling((w, h), (fit_w, fit_h), &format!(":format={out}"));
    // Centered on black, as the CPU's normalizer pads.
    if (fit_w, fit_h) != (width, height) {
        let (x, y) = centered((fit_w, fit_h), (width, height));
        steps.push(format!(
            "pad_cuda=w={width}:h={height}:x={x}:y={y}:color=black"
        ));
    }
    steps.join(",")
}

/// `scale_cuda` steps from `w`×`h` frames to `fit_w`×`fit_h` (with
/// `options` for the last). `scale_cuda` samples a fixed few pixels around
/// each output pixel, which aliases when it shrinks a lot (4K to 960 wide):
/// halvings first, each a bilinear sample between four pixels, so their
/// average, then one bicubic step to the size.
pub(crate) fn scaling(
    (w, h): (u32, u32),
    (fit_w, fit_h): (u32, u32),
    options: &str,
) -> Vec<String> {
    let mut steps = Vec::new();
    let (mut w, mut h) = (w, h);
    while w >= 2 * fit_w && h >= 2 * fit_h && w >= 4 && h >= 4 {
        (w, h) = ((w / 2) & !1, (h / 2) & !1);
        steps.push(format!("scale_cuda=w={w}:h={h}:interp_algo=bilinear"));
    }
    steps.push(format!(
        "scale_cuda=w={fit_w}:h={fit_h}{options}:interp_algo=bicubic"
    ));
    steps
}

/// Where a `fit_w`×`fit_h` picture goes in a `width`×`height` frame:
/// centered, at even offsets as the CPU's `pad` rounds them (4:2:0 chroma
/// covers two rows and columns).
fn centered((fit_w, fit_h): (u32, u32), (width, height): (u32, u32)) -> (u32, u32) {
    (((width - fit_w) / 2) & !1, ((height - fit_h) / 2) & !1)
}

/// libplacebo's options for `width`×`height` pictures stored turned by
/// `rotation` (clockwise), made `out` pictures laid out as `layout` says,
/// in `color`'s output colors. Where only the matrix and range change, as
/// swscale converts SDR on the CPU, the input is tagged with the output's
/// primaries and transfer too; for HDR, `color`'s shader converts them,
/// after scaling. It scales in the source's gamma like swscale, with the
/// cubic `scale_cuda` uses, rounds without dithering (its dither leans
/// bright), and leaves film grain and Dolby Vision metadata alone (the
/// decoder applied the grain). Peak detection is off: the CPU's tone curve
/// is fixed.
fn placebo_options(
    (width, height): (u32, u32),
    rotation: u16,
    out: &str,
    layout: Layout,
    color: &GpuColor,
) -> Vec<(&'static str, String)> {
    let mut options: Vec<(&str, String)> = match layout {
        Layout::Upright if rotation % 180 == 90 => {
            vec![("w", height.to_string()), ("h", width.to_string())]
        }
        Layout::Upright => vec![("w", width.to_string()), ("h", height.to_string())],
        // The picture fills the frame.
        Layout::Placed {
            size: (w, h),
            fitted,
            ..
        } if fitted == (w, h) => vec![("w", w.to_string()), ("h", h.to_string())],
        // Input 0 is the black frame, the whole frame; input 1 the picture.
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
            unsafe { std::ffi::CStr::from_ptr(p) }
                .to_string_lossy()
                .into_owned()
        }
    };
    let o = color.output;
    options.extend([
        ("rotate", rotation.to_string()),
        ("format", out.into()),
        (
            "colorspace",
            name(unsafe { ffi::av_color_space_name(o.matrix) }),
        ),
        (
            "color_primaries",
            name(unsafe { ffi::av_color_primaries_name(o.primaries) }),
        ),
        (
            "color_trc",
            name(unsafe { ffi::av_color_transfer_name(o.transfer) }),
        ),
        ("range", "tv".into()),
        ("upscaler", "catmull_rom".into()),
        ("downscaler", "catmull_rom".into()),
        ("disable_linear", "1".into()),
        ("sigmoid", "0".into()),
        ("dithering", "none".into()),
        ("peak_detect", "0".into()),
        ("apply_filmgrain", "0".into()),
        ("apply_dolbyvision", "0".into()),
    ]);
    if let Some(shader) = &color.shader {
        // A binary option: hexadecimal.
        let hex: String = shader.bytes().map(|b| format!("{b:02x}")).collect();
        options.push(("custom_shader_bin", hex));
    }
    if let Some(prefix) = shader_cache() {
        options.push(("shader_cache", prefix));
    }
    options
}

/// The buffer source `name` of `graph`, for `width`×`height` frames of
/// `pixel` in GPU memory from `frames`, timed in `time_base`, with sample
/// aspect `sar`. It needs to know where they are before it starts, so it is
/// made by hand.
pub(crate) fn add_source(
    graph: &mut filter::Graph,
    name: &std::ffi::CStr,
    pixel: ffi::AVPixelFormat,
    (width, height): (u32, u32),
    (time_base, sar): (ffi::AVRational, ffmpeg::Rational),
    frames: *mut ffi::AVBufferRef,
    color: Option<(ffi::AVColorSpace, ffi::AVColorRange)>,
) -> Result<()> {
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
        (*par).width = width as i32;
        (*par).height = height as i32;
        (*par).time_base = time_base;
        (*par).sample_aspect_ratio = ffi::AVRational {
            num: sar.numerator().max(1),
            den: sar.denominator().max(1),
        };
        // The source takes a reference of its own.
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

/// The buffer sink `out` of `graph`.
fn add_sink(graph: &mut filter::Graph) -> Result<()> {
    let sink = filter::find("buffersink").ok_or_else(|| Error::Invalid("no buffersink".into()))?;
    graph
        .add(&sink, "out", "")
        .ctx(|| "creating the GPU sink".into())?;
    Ok(())
}

/// The next frame out of `graph`'s sink, if one is ready.
fn take(graph: &mut filter::Graph, what: &str) -> Result<Option<(frame::Video, ffmpeg::Rational)>> {
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

impl CudaPictures {
    /// The pictures for the frames `reader` decodes on `device`, shown as
    /// its stream says (turned upright), for an output of
    /// `target`'s dynamic range (`None`: SDR BT.709). `at_once`: each
    /// frame's picture is ready as soon as the frame is in, as an export
    /// picks them one by one; otherwise one may come with the next frame,
    /// leaving the GPU less to do.
    pub fn new(
        device: &CudaDevice,
        reader: &VideoReader,
        (width, height): (u32, u32),
        format: format::Pixel,
        target: Option<Hdr>,
        at_once: bool,
    ) -> Result<Self> {
        let first = reader
            .peek()
            .ok_or_else(|| Error::Invalid("no frames".into()))?;
        let (time_base, rotation) = (reader.time_base(), reader.rotation());
        if first.format() != format::Pixel::CUDA {
            return Err(Error::Invalid("not frames in GPU memory".into()));
        }
        if !matches!(rotation, 0 | 90 | 180 | 270) {
            return Err(Error::Invalid(format!("a rotation of {rotation}°")));
        }
        let color = crate::color::gpu_color(first, target)
            .ok_or_else(|| Error::Invalid("RGB pictures".into()))?;
        let out = match format {
            format::Pixel::P010LE => "p010le",
            _ => "nv12",
        };
        // As shown: a quarter turn swaps the sides and the pixel aspect.
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
        let fitted = crate::export::video::fit(
            size.0,
            size.1,
            (shown_sar.numerator(), shown_sar.denominator()),
            width,
            height,
        );
        let decoded = unsafe { (*first.as_ptr()).hw_frames_ctx };
        if decoded.is_null() {
            return Err(Error::Invalid(
                "decoded frames without a frames context".into(),
            ));
        }
        // libplacebo converts HDR after scaling, as the CPU does, and places
        // 10-bit pictures: FFmpeg's `pad_cuda` takes no 10-bit frames.
        let placed = Layout::Placed {
            size: (width, height),
            fitted,
            format,
        };
        let layout = if color.shader.is_some()
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
                    device, decoded, stream, &color, layout, at_once,
                )?)
            }
            None => None,
        };
        // Placed pictures are done: the CUDA filters pass them on.
        let (size, spec, sar) = if let Some(Layout::Placed { .. }) = layout {
            ((width, height), "null".to_string(), ffmpeg::Rational(1, 1))
        } else {
            let spec = filters(size, fitted, (width, height, out));
            (size, spec, shown_sar)
        };
        let frames = placebo.as_ref().map_or(decoded, |p| p.output.0);
        let mut graph = filter::Graph::new();
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
        crate::parts::parse_filters(&mut graph, &spec)?;
        graph.validate().ctx(|| format!("configuring {spec}"))?;
        Ok(Self { graph, placebo })
    }

    /// The frames context of the pictures: what NVENC is opened with.
    pub fn frames_ctx(&mut self) -> *mut ffi::AVBufferRef {
        unsafe {
            ffi::av_buffersink_get_hw_frames_ctx(self.graph.get("out").expect("sink").as_mut_ptr())
        }
    }

    /// Feeds the next decoded frame, or the end of the stream (`None`).
    pub fn push(&mut self, f: Option<&frame::Video>) -> Result<()> {
        let mut source = self.graph.get("in").expect("source");
        let Some(p) = &mut self.placebo else {
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
            source
                .source()
                .add(&converted)
                .ctx(|| "scaling on the GPU".into())?;
        }
        if f.is_none() {
            source
                .source()
                .flush()
                .ctx(|| "flushing the GPU scaler".into())?;
        }
        Ok(())
    }

    /// The next picture, if one is ready: timed in the source's time base.
    pub fn pull(&mut self) -> Result<Option<frame::Video>> {
        let Some((mut out, _)) = take(&mut self.graph, "scaling on the GPU")? else {
            return Ok(None);
        };
        unsafe {
            (*out.as_mut_ptr()).sample_aspect_ratio = ffi::AVRational { num: 1, den: 1 };
        }
        Ok(Some(out))
    }
}

impl Placebo {
    /// The converter for frames of `decoded` (a CUDA frames context) of
    /// `width`×`height` with sample aspect `sar`, stored turned by
    /// `rotation`, converted as `color` says, laid out as `layout`.
    fn new(
        device: &CudaDevice,
        decoded: *mut ffi::AVBufferRef,
        ((width, height), sar, rotation): ((u32, u32), ffmpeg::Rational, u16),
        color: &GpuColor,
        layout: Layout,
        at_once: bool,
    ) -> Result<Self> {
        use ffi::AVPixelFormat::{AV_PIX_FMT_CUDA as CUDA, AV_PIX_FMT_VULKAN as VULKAN};
        let _one = converting();
        let vulkan = device.vulkan()?;
        let sw = unsafe { (*((*decoded).data as *mut ffi::AVHWFramesContext)).sw_format };
        let turned = rotation % 180 == 90;
        // Upright at its own size in the source's depth, or placed.
        let (out_size, out_pixel, out_sar) = match layout {
            Layout::Upright => {
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
        // 10-bit pictures come out as P016 too (`as_read`) and are copied
        // into P010 frames as they are: NVENC reads the top 10 bits.
        let (out, vulkan_out) = match out_pixel {
            format::Pixel::P010LE => ("p016le", format::Pixel::P016LE),
            _ => ("nv12", format::Pixel::NV12),
        };
        let input = HwFrames::new(vulkan.0, VULKAN, as_read(sw), width, height)?;
        let output = HwFrames::new(
            device.as_ptr(),
            CUDA,
            out_pixel.into(),
            out_size.0,
            out_size.1,
        )?;
        let mut graph = filter::Graph::new();
        // Black in the output's colors: a shader passes it on as black.
        let black_tags = color.output;
        let background = match layout {
            Layout::Placed {
                size: (w, h),
                fitted,
                ..
            } if fitted != (w, h) => {
                let format = vulkan_out;
                let frames = HwFrames::new(vulkan.0, VULKAN, format.into(), w, h)?;
                let timing = (COUNTED, ffmpeg::Rational(1, 1));
                let tags = Some((black_tags.matrix, black_tags.range));
                add_source(&mut graph, c"bg", VULKAN, (w, h), timing, frames.0, tags)?;
                Some(Blacks::new(frames, format, (w, h), black_tags)?)
            }
            _ => None,
        };
        let timing = (COUNTED, sar);
        let tags = color.input;
        add_source(
            &mut graph,
            c"in",
            VULKAN,
            (width, height),
            timing,
            input.0,
            Some((tags.matrix, tags.range)),
        )?;
        add_sink(&mut graph)?;
        let options = placebo_options((width, height), rotation, out, layout, color);
        // libplacebo needs the Vulkan device before it starts; given none,
        // FFmpeg would convert on the CPU in front of it.
        unsafe {
            let ctx = ffi::avfilter_graph_alloc_filter(
                graph.as_mut_ptr(),
                ffi::avfilter_get_by_name(c"libplacebo".as_ptr()),
                c"placebo".as_ptr(),
            );
            if ctx.is_null() {
                return Err(Error::Invalid("no libplacebo filter".into()));
            }
            (*ctx).hw_device_ctx = ffi::av_buffer_ref(vulkan.0);
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
            // The black frame is input 0, under the picture.
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
            // Any picture will do: a black one.
            let size = (width, height);
            Some(Blacks::new(
                input.share(),
                as_read(sw).into(),
                size,
                color.input,
            )?)
        } else {
            None
        };
        Ok(Self {
            graph,
            input,
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
        })
    }

    /// Feeds `f` (a CUDA frame), or the end of the stream (`None`).
    fn push(&mut self, f: Option<&frame::Video>) -> Result<()> {
        let _one = converting();
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
        let mut v = self.input.copy(f, "copying to Vulkan")?;
        wait_done(&v, "copying to Vulkan")?;
        unsafe {
            ffi::av_frame_copy_props(v.as_mut_ptr(), f.as_ptr());
            // No side data: the CPU converts by the tags alone. libplacebo
            // would tone-map by HDR metadata, read an ICC profile, and turn
            // pictures by their display matrices: the filter copies each
            // frame's properties to its picture, and libplacebo turns both,
            // so the turns cancel. The rotation goes in the options instead.
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

    /// `picture` at count `n`, over the black frame when there is one.
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

    /// The next converted frame, if one is ready, with its properties and
    /// timestamp, in a CUDA frame.
    fn pull(&mut self) -> Result<Option<frame::Video>> {
        let _one = converting();
        while let Some((out, tb)) = take(&mut self.graph, "converting on the GPU")? {
            let k = out
                .pts()
                .map_or(-1, |k| unsafe { ffi::av_rescale_q(k, tb.into(), COUNTED) });
            if k % 2 != 0 {
                // The filler's.
                continue;
            }
            let mut pts = None;
            while let Some(&(j, t)) = self.pts.front()
                && j <= k
            {
                self.pts.pop_front();
                pts = t;
            }
            // libplacebo is done reading the frame this picture is of
            // before the next copy (`wait_done`).
            wait_done(&out, "converting on the GPU")?;
            let mut c = self.output.copy(&out, "copying back from Vulkan")?;
            unsafe {
                ffi::av_frame_copy_props(c.as_mut_ptr(), out.as_ptr());
                (*c.as_mut_ptr()).sample_aspect_ratio = self.sar;
            }
            c.set_pts(pts);
            return Ok(Some(c));
        }
        Ok(None)
    }
}

/// Black pictures fed to libplacebo, each at most once at a time. It takes
/// a Vulkan frame at the semaphore value the frame says and gives it back at
/// the next one, so a frame it holds twice at once ends up a step ahead of
/// anything the GPU signals, and freeing it waits for that forever (an
/// export hung so on Windows). A frame goes in again once libplacebo let go
/// of it, its only reference left here; more are made while all are out.
struct Blacks {
    pool: HwFrames,
    black: frame::Video,
    tags: Tags,
    made: Vec<frame::Video>,
}

impl Blacks {
    /// Black `size` frames of `format` from `pool`, tagged `tags`.
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

    /// A reference to a black frame libplacebo doesn't hold.
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
        Ok(frame_ref(&self.made[i]))
    }
}

/// The layout libplacebo is given frames of `sw` in: P010 as P016, which is
/// laid out the same with all 16 bits used, so the frames copy as they are.
/// A Vulkan device made from a CUDA device keeps each plane in its own
/// image, and libplacebo misses the shift of such P010 planes: it reads
/// them 64 times too bright, and writes a picture it blends a 64th as
/// bright.
fn as_read(sw: ffi::AVPixelFormat) -> ffi::AVPixelFormat {
    match sw {
        ffi::AVPixelFormat::AV_PIX_FMT_P010LE => ffi::AVPixelFormat::AV_PIX_FMT_P016LE,
        other => other,
    }
}

/// A new reference to `f`'s picture and properties.
fn frame_ref(f: &frame::Video) -> frame::Video {
    let mut r = frame::Video::empty();
    unsafe { ffi::av_frame_ref(r.as_mut_ptr(), f.as_ptr()) };
    r
}

/// Tags a frame for libplacebo with `tags`, one count long.
fn tag(f: &mut frame::Video, tags: Tags) {
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
    use super::{Layout, filters, placebo_options};
    use crate::color::{GpuColor, gpu_color};
    use crate::ffmpeg::{ffi, format::Pixel, frame};
    use vidcut_core::model::Hdr;

    /// How the GPU converts a 1080p 10-bit picture of `hdr`'s transfer
    /// (`None`: SDR, BT.709) for an output of `target`'s.
    fn colors(hdr: Option<Hdr>, target: Option<Hdr>) -> GpuColor {
        use ffi::AVColorTransferCharacteristic as T;
        let mut f = frame::Video::new(Pixel::YUV420P10LE, 1920, 1080);
        unsafe {
            (*f.as_mut_ptr()).color_trc = match hdr {
                Some(Hdr::Pq) => T::AVCOL_TRC_SMPTE2084,
                Some(Hdr::Hlg) => T::AVCOL_TRC_ARIB_STD_B67,
                None => T::AVCOL_TRC_BT709,
            };
        }
        gpu_color(&f, target).unwrap()
    }

    /// Untagged SD: BT.601, into SDR.
    fn sd() -> GpuColor {
        gpu_color(&frame::Video::new(Pixel::YUV420P, 720, 480), None).unwrap()
    }

    /// The value libplacebo is given for option `key`.
    fn option<'a>(options: &'a [(&str, String)], key: &str) -> Option<&'a str> {
        options
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn big_shrinks_halve_first() {
        assert_eq!(
            filters((3840, 2160), (960, 540), (960, 540, "nv12")),
            "scale_cuda=w=1920:h=1080:interp_algo=bilinear,\
             scale_cuda=w=960:h=540:interp_algo=bilinear,\
             scale_cuda=w=960:h=540:format=nv12:interp_algo=bicubic"
        );
        assert_eq!(
            filters((1280, 720), (960, 540), (960, 540, "nv12")),
            "scale_cuda=w=960:h=540:format=nv12:interp_algo=bicubic"
        );
        // A 2.4:1 film in 16:9: fitted, then centered on black.
        assert_eq!(
            filters((1920, 800), (1920, 800), (1920, 1080, "nv12")),
            "scale_cuda=w=1920:h=800:format=nv12:interp_algo=bicubic,\
             pad_cuda=w=1920:h=1080:x=0:y=140:color=black"
        );
        // An odd margin starts a row early, on the chroma grid, as on the
        // CPU.
        assert_eq!(
            filters((1280, 534), (1280, 534), (1280, 720, "nv12")),
            "scale_cuda=w=1280:h=534:format=nv12:interp_algo=bicubic,\
             pad_cuda=w=1280:h=720:x=0:y=92:color=black"
        );
    }

    #[test]
    fn libplacebo_changes_only_the_colors() {
        let options = placebo_options((720, 480), 0, "nv12", Layout::Upright, &sd());
        let get = |k| option(&options, k);
        assert_eq!((get("w"), get("h")), (Some("720"), Some("480")));
        assert_eq!(get("rotate"), Some("0"));
        assert_eq!(get("format"), Some("nv12"));
        assert_eq!(get("colorspace"), Some("bt709"));
        assert_eq!(get("color_primaries"), Some("bt709"));
        assert_eq!(get("color_trc"), Some("bt709"));
        assert_eq!(get("range"), Some("tv"));
        assert_eq!(get("dithering"), Some("none"));
        assert_eq!(get("inputs"), None);
        assert_eq!(get("custom_shader_bin"), None);
    }

    #[test]
    fn libplacebo_turns_pictures_upright() {
        // A phone's 1920×1080 frames, stored for a quarter turn clockwise.
        let options = placebo_options((1920, 1080), 90, "nv12", Layout::Upright, &sd());
        let get = |k| option(&options, k);
        assert_eq!((get("w"), get("h")), (Some("1080"), Some("1920")));
        assert_eq!(get("rotate"), Some("90"));
        let options = placebo_options((1920, 1080), 180, "nv12", Layout::Upright, &sd());
        let get = |k| option(&options, k);
        assert_eq!((get("w"), get("h")), (Some("1920"), Some("1080")));
    }

    #[test]
    fn libplacebo_places_pictures_on_black() {
        // A 2.4:1 film in 16:9, 10 bits: the black frame is input 0 and
        // covers everything, the picture is centered over it.
        let placed = Layout::Placed {
            size: (1920, 1080),
            fitted: (1920, 800),
            format: Pixel::P010LE,
        };
        let options = placebo_options((3840, 1600), 0, "p010le", placed, &sd());
        let get = |k| option(&options, k);
        assert_eq!(get("inputs"), Some("2"));
        assert_eq!((get("w"), get("h")), (Some("1920"), Some("1080")));
        assert_eq!(get("pos_x"), Some("if(eq(idx,0),0,0)"));
        assert_eq!(get("pos_y"), Some("if(eq(idx,0),0,140)"));
        assert_eq!(get("pos_w"), Some("if(eq(idx,0),1920,1920)"));
        assert_eq!(get("pos_h"), Some("if(eq(idx,0),1080,800)"));
        // A picture that fills the frame needs no black under it.
        let filled = Layout::Placed {
            size: (1920, 1080),
            fitted: (1920, 1080),
            format: Pixel::NV12,
        };
        let options = placebo_options((3840, 2160), 0, "nv12", filled, &sd());
        let get = |k| option(&options, k);
        assert_eq!(get("inputs"), None);
        assert_eq!((get("w"), get("h")), (Some("1920"), Some("1080")));
        assert_eq!(get("pos_w"), None);
    }

    #[test]
    fn hdr_is_converted_by_the_cpus_math_in_a_shader() {
        let filled = Layout::Placed {
            size: (1920, 1080),
            fitted: (1920, 1080),
            format: Pixel::P010LE,
        };
        // PQ into SDR: the shader tone-maps, libplacebo writes BT.709.
        let pq = colors(Some(Hdr::Pq), None);
        let options = placebo_options((3840, 2160), 0, "p016le", filled, &pq);
        let get = |k| option(&options, k);
        assert_eq!(get("colorspace"), Some("bt709"));
        assert_eq!(get("color_trc"), Some("bt709"));
        assert_eq!(get("peak_detect"), Some("0"));
        let hex = get("custom_shader_bin").expect("a shader");
        let text: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        assert_eq!(Some(String::from_utf8(text).unwrap()), pq.shader);
        // SDR into HLG: written as BT.2020 HLG.
        let hlg = colors(None, Some(Hdr::Hlg));
        let options = placebo_options((1920, 1080), 0, "p016le", filled, &hlg);
        let get = |k| option(&options, k);
        assert_eq!(get("colorspace"), Some("bt2020nc"));
        assert_eq!(get("color_primaries"), Some("bt2020"));
        assert_eq!(get("color_trc"), Some("arib-std-b67"));
        assert!(get("custom_shader_bin").is_some());
    }
}
