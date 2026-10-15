//! Pictures NVDEC decodes, shown without leaving the GPU: a display's
//! graphics device (Vulkan) exports memory, CUDA imports it, and a picture's
//! planes are copied into it on the GPU, from where the display copies them
//! into its textures. Nothing goes through memory or the CPU.
//!
//! CUDA's driver library is loaded when first needed (`libcuda.so.1`, or
//! `nvcuda.dll` on Windows, which the NVIDIA driver installs and FFmpeg's
//! CUDA device already uses); the few calls here are declared as its
//! `cuda.h` has them. Memory comes as a file descriptor on Linux and as a
//! Win32 handle on Windows. NVDEC's library (`libnvcuvid.so.1`,
//! `nvcuvid.dll`, which FFmpeg's NVDEC decoding uses) says what NVDEC
//! decodes.

use std::ffi::c_void;
use std::sync::OnceLock;

use super::{CudaDevice, PlaneCopy};
use crate::error::{Error, Result};
use crate::ffmpeg::{ffi, format, frame};

/// Memory a Vulkan device exported, in the form this system passes it
/// (`VK_KHR_external_memory_fd`, `VK_KHR_external_memory_win32`).
#[cfg(unix)]
pub type Exported = std::os::fd::OwnedFd;
#[cfg(windows)]
pub type Exported = std::os::windows::io::OwnedHandle;

type CuResult = i32;
type CuDevicePtr = u64;
pub(super) type CuContext = *mut c_void;
pub(super) type CuStream = *mut c_void;
type CuExternalMemory = *mut c_void;

/// `CUDA_EXTERNAL_MEMORY_HANDLE_DESC`.
#[repr(C)]
struct HandleDesc {
    kind: u32,
    /// The union of an fd, a Win32 handle and name, and an NvSciBuf object.
    handle: [u64; 2],
    size: u64,
    flags: u32,
    reserved: [u32; 16],
}

/// `CUDA_EXTERNAL_MEMORY_BUFFER_DESC`.
#[repr(C)]
struct BufferDesc {
    offset: u64,
    size: u64,
    flags: u32,
    reserved: [u32; 16],
}

/// `CUDA_MEMCPY2D`.
#[repr(C)]
struct Memcpy2d {
    src_x_in_bytes: usize,
    src_y: usize,
    src_memory_type: u32,
    src_host: *const c_void,
    src_device: CuDevicePtr,
    src_array: *mut c_void,
    src_pitch: usize,
    dst_x_in_bytes: usize,
    dst_y: usize,
    dst_memory_type: u32,
    dst_host: *mut c_void,
    dst_device: CuDevicePtr,
    dst_array: *mut c_void,
    dst_pitch: usize,
    width_in_bytes: usize,
    height: usize,
}

/// `CU_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD`, `_OPAQUE_WIN32`.
#[cfg(unix)]
const HANDLE_TYPE: u32 = 1;
#[cfg(windows)]
const HANDLE_TYPE: u32 = 2;
const DEDICATED: u32 = 1;
const MEMORY_TYPE_DEVICE: u32 = 2;
/// `CU_STREAM_NON_BLOCKING`: no implicit waits for the default stream.
const STREAM_NON_BLOCKING: u32 = 1;

/// The driver's calls used here and by `cuda::mix`.
pub(super) struct Cuda {
    _lib: libloading::Library,
    init: unsafe extern "C" fn(u32) -> CuResult,
    device_get_count: unsafe extern "C" fn(*mut i32) -> CuResult,
    device_get: unsafe extern "C" fn(*mut i32, i32) -> CuResult,
    device_get_name: unsafe extern "C" fn(*mut std::ffi::c_char, i32, i32) -> CuResult,
    device_get_pci_bus_id: unsafe extern "C" fn(*mut std::ffi::c_char, i32, i32) -> CuResult,
    ctx_push: unsafe extern "C" fn(CuContext) -> CuResult,
    ctx_pop: unsafe extern "C" fn(*mut CuContext) -> CuResult,
    ctx_get_device: unsafe extern "C" fn(*mut i32) -> CuResult,
    device_get_uuid: unsafe extern "C" fn(*mut [u8; 16], i32) -> CuResult,
    device_total_mem: unsafe extern "C" fn(*mut usize, i32) -> CuResult,
    import: unsafe extern "C" fn(*mut CuExternalMemory, *const HandleDesc) -> CuResult,
    mapped_buffer:
        unsafe extern "C" fn(*mut CuDevicePtr, CuExternalMemory, *const BufferDesc) -> CuResult,
    destroy: unsafe extern "C" fn(CuExternalMemory) -> CuResult,
    free: unsafe extern "C" fn(CuDevicePtr) -> CuResult,
    copy_2d: unsafe extern "C" fn(*const Memcpy2d, CuStream) -> CuResult,
    synchronize: unsafe extern "C" fn(CuStream) -> CuResult,
    stream_create: unsafe extern "C" fn(*mut CuStream, u32) -> CuResult,
    stream_destroy: unsafe extern "C" fn(CuStream) -> CuResult,
    pub(super) module_load_data: unsafe extern "C" fn(*mut *mut c_void, *const c_void) -> CuResult,
    pub(super) module_get_function:
        unsafe extern "C" fn(*mut *mut c_void, *mut c_void, *const std::ffi::c_char) -> CuResult,
    #[allow(clippy::type_complexity)]
    pub(super) launch_kernel: unsafe extern "C" fn(
        *mut c_void,
        u32,
        u32,
        u32,
        u32,
        u32,
        u32,
        u32,
        CuStream,
        *mut *mut c_void,
        *mut *mut c_void,
    ) -> CuResult,
}

impl Cuda {
    pub(super) fn get() -> Result<&'static Cuda> {
        static CUDA: OnceLock<std::result::Result<Cuda, String>> = OnceLock::new();
        CUDA.get_or_init(|| unsafe { Self::load() }.map_err(|e| format!("CUDA's driver: {e}")))
            .as_ref()
            .map_err(|e| Error::Invalid(e.clone()))
    }

    /// # Safety
    /// Loads the driver library and takes its calls as `cuda.h` declares
    /// them.
    unsafe fn load() -> std::result::Result<Self, libloading::Error> {
        unsafe {
            #[cfg(unix)]
            let lib = libloading::Library::new("libcuda.so.1")?;
            #[cfg(windows)]
            let lib = libloading::Library::new("nvcuda.dll")?;
            macro_rules! call {
                ($name:literal) => {
                    *lib.get(concat!($name, "\0").as_bytes())?
                };
            }
            Ok(Self {
                init: call!("cuInit"),
                device_get_count: call!("cuDeviceGetCount"),
                device_get: call!("cuDeviceGet"),
                device_get_name: call!("cuDeviceGetName"),
                device_get_pci_bus_id: call!("cuDeviceGetPCIBusId"),
                ctx_push: call!("cuCtxPushCurrent_v2"),
                ctx_pop: call!("cuCtxPopCurrent_v2"),
                ctx_get_device: call!("cuCtxGetDevice"),
                device_get_uuid: call!("cuDeviceGetUuid_v2"),
                device_total_mem: call!("cuDeviceTotalMem_v2"),
                import: call!("cuImportExternalMemory"),
                mapped_buffer: call!("cuExternalMemoryGetMappedBuffer"),
                destroy: call!("cuDestroyExternalMemory"),
                free: call!("cuMemFree_v2"),
                copy_2d: call!("cuMemcpy2DAsync_v2"),
                synchronize: call!("cuStreamSynchronize"),
                stream_create: call!("cuStreamCreate"),
                stream_destroy: call!("cuStreamDestroy_v2"),
                module_load_data: call!("cuModuleLoadData"),
                module_get_function: call!("cuModuleGetFunction"),
                launch_kernel: call!("cuLaunchKernel"),
                _lib: lib,
            })
        }
    }
}

pub(super) fn check(what: &str, result: CuResult) -> Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "CUDA: {what} failed (error {result})"
        )))
    }
}

/// FFmpeg's CUDA device context behind a hardware device reference.
///
/// # Safety
/// `device` is a reference to a CUDA hardware device context.
pub(super) unsafe fn cuda_context(device: *mut ffi::AVBufferRef) -> (CuContext, CuStream) {
    unsafe {
        let ctx = (*device).data as *const ffi::AVHWDeviceContext;
        let hw = (*ctx).hwctx as *const ffi::AVCUDADeviceContext;
        ((*hw).cuda_ctx as CuContext, (*hw).stream as CuStream)
    }
}

/// Runs `f` with `ctx` current on this thread.
pub(super) fn in_context<T>(
    cuda: &Cuda,
    ctx: CuContext,
    f: impl FnOnce() -> Result<T>,
) -> Result<T> {
    check("making the context current", unsafe {
        (cuda.ctx_push)(ctx)
    })?;
    let result = f();
    let mut popped = std::ptr::null_mut();
    unsafe { (cuda.ctx_pop)(&mut popped) };
    result
}

impl CudaDevice {
    /// The GPU's UUID, as Vulkan reports it too (`deviceUUID`): a display
    /// on the same GPU can share memory with it.
    pub fn uuid(&self) -> Result<[u8; 16]> {
        let cuda = Cuda::get()?;
        let (ctx, _) = unsafe { cuda_context(self.as_ptr()) };
        in_context(cuda, ctx, || {
            let mut device = 0;
            check("finding the device", unsafe {
                (cuda.ctx_get_device)(&mut device)
            })?;
            let mut uuid = [0u8; 16];
            check("reading the device's UUID", unsafe {
                (cuda.device_get_uuid)(&mut uuid, device)
            })?;
            Ok(uuid)
        })
    }

    /// The GPU's memory in bytes.
    pub fn memory(&self) -> Result<usize> {
        let cuda = Cuda::get()?;
        let (ctx, _) = unsafe { cuda_context(self.as_ptr()) };
        in_context(cuda, ctx, || {
            let mut device = 0;
            check("finding the device", unsafe {
                (cuda.ctx_get_device)(&mut device)
            })?;
            let mut bytes = 0;
            check("reading the device's memory", unsafe {
                (cuda.device_total_mem)(&mut bytes, device)
            })?;
            Ok(bytes)
        })
    }
}

/// A CUDA device: its index, which FFmpeg opens it by, its name, its PCI
/// slot and its UUID.
pub(crate) struct Listed {
    pub index: i32,
    pub name: String,
    pub slot: Option<crate::gpus::Slot>,
    pub uuid: Option<[u8; 16]>,
    /// Its memory in bytes.
    pub memory: Option<usize>,
}

/// The CUDA devices, in CUDA's order; none without NVIDIA's driver.
pub(crate) fn listed() -> Vec<Listed> {
    let Ok(cuda) = Cuda::get() else {
        return Vec::new();
    };
    let text = |buf: &[std::ffi::c_char]| {
        // SAFETY: the driver wrote a NUL-terminated string into `buf`,
        // which holds a NUL at its end anyway.
        unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    };
    // SAFETY: the driver's calls as `cuda.h` declares them, each writing
    // into what it is given, of the length given.
    unsafe {
        let mut count = 0;
        if (cuda.init)(0) != 0 || (cuda.device_get_count)(&mut count) != 0 {
            return Vec::new();
        }
        (0..count)
            .filter_map(|index| {
                let mut device = 0;
                if (cuda.device_get)(&mut device, index) != 0 {
                    return None;
                }
                let mut name = [0 as std::ffi::c_char; 256];
                (cuda.device_get_name)(name.as_mut_ptr(), 255, device);
                let mut bus = [0 as std::ffi::c_char; 64];
                let slot = ((cuda.device_get_pci_bus_id)(bus.as_mut_ptr(), 63, device) == 0)
                    .then(|| crate::gpus::parse_slot(&text(&bus)))
                    .flatten();
                let mut uuid = [0u8; 16];
                let uuid = ((cuda.device_get_uuid)(&mut uuid, device) == 0).then_some(uuid);
                let mut bytes = 0;
                let memory = ((cuda.device_total_mem)(&mut bytes, device) == 0).then_some(bytes);
                Some(Listed {
                    index,
                    name: text(&name),
                    slot,
                    uuid,
                    memory,
                })
            })
            .collect()
    }
}

/// Waits until the work queued on the stream of `frame`'s device is done,
/// for a frame in CUDA memory.
pub(crate) fn wait_for(frame: &frame::Video) -> Result<()> {
    if frame.format() != format::Pixel::CUDA {
        return Ok(());
    }
    let cuda = Cuda::get()?;
    // SAFETY: a CUDA frame's frames context is on a CUDA device.
    let (ctx, stream) = unsafe {
        let frames = (*frame.as_ptr()).hw_frames_ctx;
        if frames.is_null() {
            return Ok(());
        }
        cuda_context((*((*frames).data as *const ffi::AVHWFramesContext)).device_ref)
    };
    in_context(cuda, ctx, || {
        check("waiting for the GPU", unsafe { (cuda.synchronize)(stream) })
    })
}

/// A stream of a CUDA device's context, destroyed when dropped.
pub(crate) struct Stream {
    cuda: &'static Cuda,
    ctx: CuContext,
    raw: CuStream,
}

impl Stream {
    /// A new stream on `device`'s context, which doesn't wait for the
    /// context's default stream.
    pub(crate) fn new(device: &CudaDevice) -> Result<Self> {
        let cuda = Cuda::get()?;
        let (ctx, _) = unsafe { cuda_context(device.as_ptr()) };
        let raw = in_context(cuda, ctx, || {
            let mut raw = std::ptr::null_mut();
            check("making a stream", unsafe {
                (cuda.stream_create)(&mut raw, STREAM_NON_BLOCKING)
            })?;
            Ok(raw)
        })?;
        Ok(Self { cuda, ctx, raw })
    }

    pub(crate) fn as_raw(&self) -> CuStream {
        self.raw
    }

    /// Lets go of it, for [`destroy_stream`] to destroy.
    pub(crate) fn into_raw(self) -> *mut c_void {
        let raw = self.raw;
        std::mem::forget(self);
        raw
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = in_context(self.cuda, self.ctx, || {
            check("destroying a stream", unsafe {
                (self.cuda.stream_destroy)(self.raw)
            })
        });
    }
}

/// Destroys the stream a device of `CudaDevice::with_own_stream` keeps in
/// its `user_opaque`: FFmpeg calls it as the device goes.
///
/// # Safety
/// `device` is such a device, its context still there.
pub(crate) unsafe extern "C" fn destroy_stream(device: *mut ffi::AVHWDeviceContext) {
    let Ok(cuda) = Cuda::get() else { return };
    // SAFETY: the device's CUDA context, and the stream made with it.
    let (ctx, stream) = unsafe {
        let hw = (*device).hwctx as *const ffi::AVCUDADeviceContext;
        (
            (*hw).cuda_ctx as CuContext,
            (*device).user_opaque as CuStream,
        )
    };
    let _ = in_context(cuda, ctx, || {
        check("destroying a stream", unsafe {
            (cuda.stream_destroy)(stream)
        })
    });
}

/// Memory a display's Vulkan device exported, as CUDA sees it: pictures
/// NVDEC decoded are copied into it on the GPU.
pub struct SharedBuffer {
    cuda: &'static Cuda,
    ctx: CuContext,
    stream: CuStream,
    memory: CuExternalMemory,
    ptr: CuDevicePtr,
    size: u64,
    /// Keeps the CUDA context alive.
    _device: CudaDevice,
}

// SAFETY: CUDA's handles may be used from any thread with their context
// made current, which every call here does.
unsafe impl Send for SharedBuffer {}

impl SharedBuffer {
    /// Imports `size` bytes of memory a Vulkan device exported as an opaque
    /// file descriptor (`VK_KHR_external_memory_fd`), or on Windows an
    /// opaque Win32 handle (`VK_KHR_external_memory_win32`), into
    /// `device`'s context. CUDA owns a descriptor from then on; a Win32
    /// handle stays the caller's, and is closed here once CUDA holds the
    /// memory.
    ///
    /// # Safety
    /// `exported` must be memory of at least `size` bytes, exported by a
    /// Vulkan device on `device`'s GPU (see [`CudaDevice::uuid`]), from a
    /// dedicated allocation when `dedicated`. The memory must stay allocated
    /// while this lives.
    pub unsafe fn import(
        device: &CudaDevice,
        exported: Exported,
        size: u64,
        dedicated: bool,
    ) -> Result<Self> {
        let cuda = Cuda::get()?;
        let (ctx, stream) = unsafe { cuda_context(device.as_ptr()) };
        in_context(cuda, ctx, || {
            #[cfg(unix)]
            let handle = {
                use std::os::fd::AsRawFd;
                [u64::from(exported.as_raw_fd() as u32), 0]
            };
            #[cfg(windows)]
            let handle = {
                use std::os::windows::io::AsRawHandle;
                [exported.as_raw_handle() as u64, 0]
            };
            let desc = HandleDesc {
                kind: HANDLE_TYPE,
                handle,
                size,
                flags: if dedicated { DEDICATED } else { 0 },
                reserved: [0; 16],
            };
            let mut memory = std::ptr::null_mut();
            // Not taken, the descriptor is still ours to close: dropped.
            check("importing shared memory", unsafe {
                (cuda.import)(&mut memory, &desc)
            })?;
            // Taken: a descriptor is CUDA's now. A Win32 handle isn't, and
            // CUDA keeps a reference of its own.
            #[cfg(unix)]
            std::mem::forget(exported);
            #[cfg(windows)]
            drop(exported);
            let buffer = BufferDesc {
                offset: 0,
                size,
                flags: 0,
                reserved: [0; 16],
            };
            let mut ptr = 0;
            if let Err(e) = check("mapping shared memory", unsafe {
                (cuda.mapped_buffer)(&mut ptr, memory, &buffer)
            }) {
                unsafe { (cuda.destroy)(memory) };
                return Err(e);
            }
            Ok(Self {
                cuda,
                ctx,
                stream,
                memory,
                ptr,
                size,
                _device: device.clone(),
            })
        })
    }

    /// Bytes it holds.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Copies the planes of `frame`, a frame in CUDA memory, into the
    /// buffer as `planes` place them (`planes[i]` for the frame's plane
    /// `i`), and waits until the copy is done: the display may read them
    /// from then on.
    pub fn copy_planes(&self, frame: &frame::Video, planes: &[PlaneCopy]) -> Result<()> {
        if frame.format() != format::Pixel::CUDA {
            return Err(Error::Invalid("the picture isn't in CUDA memory".into()));
        }
        for p in planes {
            let end = p.offset + p.pitch * p.rows.saturating_sub(1) + p.row_bytes;
            if p.row_bytes > p.pitch || end > self.size {
                return Err(Error::Invalid(format!(
                    "a plane of {} rows of {} bytes doesn't fit the shared buffer",
                    p.rows, p.row_bytes
                )));
            }
        }
        // SAFETY: a CUDA frame's planes are device pointers, `linesize`
        // bytes apart.
        let (data, linesize) = unsafe {
            let f = &*frame.as_ptr();
            (f.data, f.linesize)
        };
        in_context(self.cuda, self.ctx, || {
            for (i, p) in planes.iter().enumerate() {
                let src = data.get(i).copied().unwrap_or(std::ptr::null_mut());
                let pitch = linesize.get(i).copied().unwrap_or(0);
                if src.is_null() || pitch < 0 || (pitch as u64) < p.row_bytes {
                    return Err(Error::Invalid(format!("the picture has no plane {i}")));
                }
                let copy = Memcpy2d {
                    src_x_in_bytes: 0,
                    src_y: 0,
                    src_memory_type: MEMORY_TYPE_DEVICE,
                    src_host: std::ptr::null(),
                    src_device: src as CuDevicePtr,
                    src_array: std::ptr::null_mut(),
                    src_pitch: pitch as usize,
                    dst_x_in_bytes: 0,
                    dst_y: 0,
                    dst_memory_type: MEMORY_TYPE_DEVICE,
                    dst_host: std::ptr::null_mut(),
                    dst_device: self.ptr + p.offset,
                    dst_array: std::ptr::null_mut(),
                    dst_pitch: p.pitch as usize,
                    width_in_bytes: p.row_bytes as usize,
                    height: p.rows as usize,
                };
                // On FFmpeg's stream: after NVDEC's own copy into the frame.
                check("copying a plane", unsafe {
                    (self.cuda.copy_2d)(&copy, self.stream)
                })?;
            }
            check("waiting for the copy", unsafe {
                (self.cuda.synchronize)(self.stream)
            })
        })
    }
}

impl Drop for SharedBuffer {
    fn drop(&mut self) {
        let _ = in_context(self.cuda, self.ctx, || {
            unsafe {
                (self.cuda.free)(self.ptr);
                (self.cuda.destroy)(self.memory);
            }
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_structs_are_laid_out_as_cuda_h_has_them() {
        assert_eq!(std::mem::size_of::<HandleDesc>(), 104);
        assert_eq!(std::mem::offset_of!(HandleDesc, handle), 8);
        assert_eq!(std::mem::offset_of!(HandleDesc, size), 24);
        assert_eq!(std::mem::offset_of!(HandleDesc, flags), 32);
        assert_eq!(std::mem::size_of::<BufferDesc>(), 88);
        assert_eq!(std::mem::size_of::<Memcpy2d>(), 128);
        assert_eq!(std::mem::offset_of!(Memcpy2d, src_device), 32);
        assert_eq!(std::mem::offset_of!(Memcpy2d, dst_device), 88);
    }
}
