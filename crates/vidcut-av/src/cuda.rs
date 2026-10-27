#![warn(clippy::undocumented_unsafe_blocks)]

pub(crate) mod keep;
pub mod share;

pub(crate) use share::{Listed, listed};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaneCopy {
    pub offset: u64,
    pub pitch: u64,
    pub row_bytes: u64,
    pub rows: u64,
}

use std::ptr;
use std::sync::{Arc, Mutex, OnceLock};

use crate::error::{Error, Result};
use crate::ffmpeg::{self, ffi, format, frame};
use crate::hw::{Device, GpuGraph};

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

    fn fresh(&self) -> Result<frame::Video> {
        let mut hw = frame::Video::empty();
        // SAFETY: a fresh frame of this live pool.
        let ret = unsafe { ffi::av_hwframe_get_buffer(self.0, hw.as_mut_ptr(), 0) };
        if ret < 0 {
            return Err(Error::Ffmpeg {
                context: "getting a GPU frame".into(),
                source: ffmpeg::Error::from(ret),
            });
        }
        Ok(hw)
    }

    fn copy(&self, src: &frame::Video, what: &str) -> Result<frame::Video> {
        let mut hw = self.fresh()?;
        // SAFETY: a frame of this live pool, given `src`'s picture.
        unsafe {
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

    pub fn fresh(&self) -> Result<frame::Video> {
        self.0.fresh()
    }

    pub fn upload(&self, src: &frame::Video) -> Result<frame::Video> {
        let mut hw = self.0.copy(src, "uploading to the GPU")?;
        crate::copies::up(src);
        hw.set_pts(src.pts());
        Ok(hw)
    }
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

pub(crate) fn add_source(
    graph: &mut GpuGraph,
    name: &std::ffi::CStr,
    pixel: ffi::AVPixelFormat,
    (width, height): (u32, u32),
    (time_base, sar): (ffi::AVRational, ffmpeg::Rational),
    frames: *mut ffi::AVBufferRef,
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

#[cfg(test)]
mod tests {
    use crate::ffmpeg::{self, ffi};

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
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "1920×2147483648 doesn't fit a frame");
    }
}
