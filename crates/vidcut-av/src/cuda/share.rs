use std::ffi::c_void;
use std::sync::OnceLock;

use super::{CudaDevice, PlaneCopy};
use crate::error::{Error, Result};
use crate::ffmpeg::{ffi, format, frame};

#[cfg(unix)]
pub type Exported = std::os::fd::OwnedFd;
#[cfg(windows)]
pub type Exported = std::os::windows::io::OwnedHandle;

type CuResult = i32;
pub(super) type CuDevicePtr = u64;
pub(super) type CuContext = *mut c_void;
pub(super) type CuStream = *mut c_void;
type CuExternalMemory = *mut c_void;

#[repr(C)]
struct HandleDesc {
    kind: u32,
    handle: [u64; 2],
    size: u64,
    flags: u32,
    reserved: [u32; 16],
}

#[repr(C)]
struct BufferDesc {
    offset: u64,
    size: u64,
    flags: u32,
    reserved: [u32; 16],
}

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

#[cfg(unix)]
const HANDLE_TYPE: u32 = 1;
#[cfg(windows)]
const HANDLE_TYPE: u32 = 2;
const DEDICATED: u32 = 1;
const MEMORY_TYPE_DEVICE: u32 = 2;
const STREAM_NON_BLOCKING: u32 = 1;

pub(super) struct Cuda {
    _lib: libloading::Library,
    init: unsafe extern "C" fn(u32) -> CuResult,
    device_get_count: unsafe extern "C" fn(*mut i32) -> CuResult,
    device_get: unsafe extern "C" fn(*mut i32, i32) -> CuResult,
    device_get_name: unsafe extern "C" fn(*mut std::ffi::c_char, i32, i32) -> CuResult,
    device_get_pci_bus_id: unsafe extern "C" fn(*mut std::ffi::c_char, i32, i32) -> CuResult,
    ctx_push: unsafe extern "C" fn(CuContext) -> CuResult,
    ctx_pop: unsafe extern "C" fn(*mut CuContext) -> CuResult,
    pub(super) ctx_get_device: unsafe extern "C" fn(*mut i32) -> CuResult,
    device_get_uuid: unsafe extern "C" fn(*mut [u8; 16], i32) -> CuResult,
    pub(super) device_get_attribute: unsafe extern "C" fn(*mut i32, i32, i32) -> CuResult,
    import: unsafe extern "C" fn(*mut CuExternalMemory, *const HandleDesc) -> CuResult,
    mapped_buffer:
        unsafe extern "C" fn(*mut CuDevicePtr, CuExternalMemory, *const BufferDesc) -> CuResult,
    destroy: unsafe extern "C" fn(CuExternalMemory) -> CuResult,
    free: unsafe extern "C" fn(CuDevicePtr) -> CuResult,
    copy_2d: unsafe extern "C" fn(*const Memcpy2d, CuStream) -> CuResult,
    synchronize: unsafe extern "C" fn(CuStream) -> CuResult,
    stream_create: unsafe extern "C" fn(*mut CuStream, u32) -> CuResult,
    stream_destroy: unsafe extern "C" fn(CuStream) -> CuResult,
    pub(super) alloc_async:
        Option<unsafe extern "C" fn(*mut CuDevicePtr, usize, CuStream) -> CuResult>,
    pub(super) free_async: Option<unsafe extern "C" fn(CuDevicePtr, CuStream) -> CuResult>,
}

impl Cuda {
    pub(super) fn get() -> Result<&'static Cuda> {
        static CUDA: OnceLock<std::result::Result<Cuda, String>> = OnceLock::new();
        // SAFETY: loaded once and kept for the process.
        CUDA.get_or_init(|| unsafe { Self::load() }.map_err(|e| format!("CUDA's driver: {e}")))
            .as_ref()
            .map_err(|e| Error::Invalid(e.clone()))
    }

    unsafe fn load() -> std::result::Result<Self, libloading::Error> {
        // SAFETY: the driver's calls as `cuda.h` declares them; `_lib` keeps them loaded.
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
                device_get_attribute: call!("cuDeviceGetAttribute"),
                import: call!("cuImportExternalMemory"),
                mapped_buffer: call!("cuExternalMemoryGetMappedBuffer"),
                destroy: call!("cuDestroyExternalMemory"),
                free: call!("cuMemFree_v2"),
                copy_2d: call!("cuMemcpy2DAsync_v2"),
                synchronize: call!("cuStreamSynchronize"),
                stream_create: call!("cuStreamCreate"),
                stream_destroy: call!("cuStreamDestroy_v2"),
                alloc_async: lib.get(b"cuMemAllocAsync\0").ok().map(|f| *f),
                free_async: lib.get(b"cuMemFreeAsync\0").ok().map(|f| *f),
                _lib: lib,
            })
        }
    }
}

impl Cuda {
    pub(super) unsafe fn copy_rows(
        &self,
        (from, from_pitch): (CuDevicePtr, usize),
        (to, to_pitch): (CuDevicePtr, usize),
        (row_bytes, rows): (usize, usize),
        stream: CuStream,
    ) -> CuResult {
        let copy = Memcpy2d {
            src_x_in_bytes: 0,
            src_y: 0,
            src_memory_type: MEMORY_TYPE_DEVICE,
            src_host: std::ptr::null(),
            src_device: from,
            src_array: std::ptr::null_mut(),
            src_pitch: from_pitch,
            dst_x_in_bytes: 0,
            dst_y: 0,
            dst_memory_type: MEMORY_TYPE_DEVICE,
            dst_host: std::ptr::null_mut(),
            dst_device: to,
            dst_array: std::ptr::null_mut(),
            dst_pitch: to_pitch,
            width_in_bytes: row_bytes,
            height: rows,
        };
        // SAFETY: as the caller says.
        unsafe { (self.copy_2d)(&copy, stream) }
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

fn both<T>(work: Result<T>, next: Result<()>) -> Result<T> {
    match (work, next) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(error)) | (Err(error), Ok(())) => Err(error),
        (Err(work), Err(next)) => Err(Error::Invalid(format!("{work}; {next}"))),
    }
}

fn debug_status(result: Result<()>) {
    if let Err(error) = result {
        crate::debug!("decoding", "{error}");
    }
}

pub(super) unsafe fn cuda_context(device: *mut ffi::AVBufferRef) -> (CuContext, CuStream) {
    // SAFETY: a CUDA device, as the caller says, whose `hwctx` is an `AVCUDADeviceContext`.
    unsafe {
        let ctx = (*device).data as *const ffi::AVHWDeviceContext;
        let hw = (*ctx).hwctx as *const ffi::AVCUDADeviceContext;
        ((*hw).cuda_ctx as CuContext, (*hw).stream as CuStream)
    }
}

struct PushedContext {
    pop: unsafe extern "C" fn(*mut CuContext) -> CuResult,
}

impl PushedContext {
    fn leave(self) -> Result<()> {
        let mut popped = std::ptr::null_mut();
        // SAFETY: as in `drop`, which the guard then skips: it pops once.
        let status = unsafe { (self.pop)(&mut popped) };
        std::mem::forget(self);
        check("leaving the context", status)
    }
}

impl Drop for PushedContext {
    fn drop(&mut self) {
        let mut popped = std::ptr::null_mut();
        // SAFETY: `pop` is `cuCtxPopCurrent_v2`; `in_context` pushed the context on this thread.
        unsafe { (self.pop)(&mut popped) };
    }
}

pub(super) fn in_context<T>(
    cuda: &Cuda,
    ctx: CuContext,
    f: impl FnOnce() -> Result<T>,
) -> Result<T> {
    // SAFETY: every caller passes a live device's context.
    check("making the context current", unsafe {
        (cuda.ctx_push)(ctx)
    })?;
    let current = PushedContext { pop: cuda.ctx_pop };
    let result = f();
    both(result, current.leave())
}

impl CudaDevice {
    pub fn uuid(&self) -> Result<[u8; 16]> {
        let cuda = Cuda::get()?;
        // SAFETY: a CUDA device, as `cuda_context` asks.
        let (ctx, _) = unsafe { cuda_context(self.as_ptr()) };
        in_context(cuda, ctx, || {
            let mut device = 0;
            // SAFETY: the context is current; the ordinal goes into `device`.
            check("finding the device", unsafe {
                (cuda.ctx_get_device)(&mut device)
            })?;
            let mut uuid = [0u8; 16];
            // SAFETY: the ordinal just read; `uuid` is 16 bytes, as `CUuuid` is.
            check("reading the device's UUID", unsafe {
                (cuda.device_get_uuid)(&mut uuid, device)
            })?;
            Ok(uuid)
        })
    }
}

pub(crate) struct Listed {
    pub index: i32,
    pub name: String,
    pub slot: Option<crate::gpus::Slot>,
    pub uuid: Option<[u8; 16]>,
}

pub(crate) fn listed() -> Vec<Listed> {
    let Ok(cuda) = Cuda::get() else {
        return Vec::new();
    };
    let text = |buf: &[std::ffi::c_char]| {
        // SAFETY: `buf` ends in a NUL the driver is never given, so the string ends within it.
        unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    };
    // SAFETY: the driver's calls as `cuda.h` declares them, each writing within the length given.
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
                if (cuda.device_get_name)(name.as_mut_ptr(), 255, device) != 0 {
                    return None;
                }
                let mut bus = [0 as std::ffi::c_char; 64];
                let slot = ((cuda.device_get_pci_bus_id)(bus.as_mut_ptr(), 63, device) == 0)
                    .then(|| crate::gpus::parse_slot(&text(&bus)))
                    .flatten();
                let mut uuid = [0u8; 16];
                let uuid = ((cuda.device_get_uuid)(&mut uuid, device) == 0).then_some(uuid);
                Some(Listed {
                    index,
                    name: text(&name),
                    slot,
                    uuid,
                })
            })
            .collect()
    }
}

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
        // SAFETY: the context is current; `stream` is its device's (null: the default one).
        check("waiting for the GPU", unsafe { (cuda.synchronize)(stream) })
    })
}

pub(crate) struct Stream {
    cuda: &'static Cuda,
    ctx: CuContext,
    raw: CuStream,
}

impl Stream {
    pub(crate) fn new(device: &CudaDevice) -> Result<Self> {
        let cuda = Cuda::get()?;
        // SAFETY: a CUDA device, as `cuda_context` asks.
        let (ctx, _) = unsafe { cuda_context(device.as_ptr()) };
        let raw = in_context(cuda, ctx, || {
            let mut raw = std::ptr::null_mut();
            // SAFETY: the context is current; the stream goes into `raw`.
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

    pub(crate) fn into_raw(self) -> *mut c_void {
        let raw = self.raw;
        std::mem::forget(self);
        raw
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        debug_status(in_context(self.cuda, self.ctx, || {
            // SAFETY: the context is current; the stream this value made.
            check("destroying a stream", unsafe {
                (self.cuda.stream_destroy)(self.raw)
            })
        }));
    }
}

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
    debug_status(in_context(cuda, ctx, || {
        // SAFETY: the context is current; the stream made for this device.
        check("destroying a stream", unsafe {
            (cuda.stream_destroy)(stream)
        })
    }));
}

fn copy_len(n: u64) -> Result<usize> {
    usize::try_from(n).map_err(|_| Error::Invalid(format!("{n} doesn't fit a copy")))
}

fn source_pitch(pitch: i32) -> Result<usize> {
    usize::try_from(pitch)
        .map_err(|_| Error::Invalid(format!("a picture stride of {pitch} doesn't fit")))
}

pub struct SharedBuffer {
    cuda: &'static Cuda,
    ctx: CuContext,
    stream: CuStream,
    memory: CuExternalMemory,
    ptr: CuDevicePtr,
    size: u64,
    _device: CudaDevice,
}

// SAFETY: CUDA's handles may be used on any thread; every call here makes their context current.
unsafe impl Send for SharedBuffer {}

impl SharedBuffer {
    /// # Safety
    /// `exported`: `size`+ bytes from Vulkan on `device`'s GPU, dedicated if `dedicated`, kept while this lives.
    pub unsafe fn import(
        device: &CudaDevice,
        exported: Exported,
        size: u64,
        dedicated: bool,
    ) -> Result<Self> {
        let cuda = Cuda::get()?;
        // SAFETY: a CUDA device, as `cuda_context` asks.
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
            // SAFETY: the context is current; `desc` has `CUDA_EXTERNAL_MEMORY_HANDLE_DESC`'s layout.
            check("importing shared memory", unsafe {
                (cuda.import)(&mut memory, &desc)
            })?;
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
            // SAFETY: the context is current; `memory` is the import just made.
            if let Err(e) = check("mapping shared memory", unsafe {
                (cuda.mapped_buffer)(&mut ptr, memory, &buffer)
            }) {
                return both(
                    Err(e),
                    // SAFETY: the import just made, which nothing else holds.
                    check("destroying shared memory", unsafe {
                        (cuda.destroy)(memory)
                    }),
                );
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

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn copy_planes(&self, frame: &frame::Video, planes: &[PlaneCopy]) -> Result<()> {
        self.copied(frame, planes, false)
    }

    pub fn copy_into(&self, frame: &frame::Video, planes: &[PlaneCopy]) -> Result<()> {
        self.copied(frame, planes, true)
    }

    fn copied(&self, frame: &frame::Video, planes: &[PlaneCopy], into: bool) -> Result<()> {
        if frame.format() != format::Pixel::CUDA {
            return Err(Error::Invalid("the picture isn't in CUDA memory".into()));
        }
        planes_fit(self.size, frame, planes)?;
        // SAFETY: `frame` is a CUDA frame: its planes are device pointers, rows `linesize` apart.
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
                let picture = (src as CuDevicePtr, source_pitch(pitch)?);
                let shared = (self.ptr + p.offset, copy_len(p.pitch)?);
                let (from, to) = if into {
                    (shared, picture)
                } else {
                    (picture, shared)
                };
                let row_bytes = copy_len(p.row_bytes)?;
                let rows = copy_len(p.rows)?;
                // SAFETY: the context is current; plane `i` fits `frame` and the buffer, as checked.
                check("copying a plane", unsafe {
                    self.cuda
                        .copy_rows(from, to, (row_bytes, rows), self.stream)
                })?;
            }
            // SAFETY: the context is current; this buffer's stream.
            check("waiting for the copy", unsafe {
                (self.cuda.synchronize)(self.stream)
            })
        })
    }
}

fn planes_fit(size: u64, frame: &frame::Video, planes: &[PlaneCopy]) -> Result<()> {
    let layout = crate::hw::sw_format(frame);
    let height = frame.height();
    for (i, p) in planes.iter().enumerate() {
        let end = p
            .pitch
            .checked_mul(p.rows.saturating_sub(1))
            .and_then(|span| span.checked_add(p.row_bytes))
            .and_then(|span| p.offset.checked_add(span));
        if p.row_bytes > p.pitch || end.is_none_or(|end| end > size) {
            return Err(Error::Invalid(format!(
                "a plane of {} rows of {} bytes doesn't fit the shared buffer",
                p.rows, p.row_bytes
            )));
        }
        let rows = plane_rows(layout, height, i);
        if p.rows > u64::from(rows) {
            return Err(Error::Invalid(format!(
                "a plane of {} rows is past the picture's plane of {rows}",
                p.rows
            )));
        }
    }
    Ok(())
}

fn plane_rows(layout: format::Pixel, height: u32, index: usize) -> u32 {
    if index != 1 && index != 2 {
        return height;
    }
    let shift = layout.descriptor().map(|d| d.log2_chroma_h()).unwrap_or(0);
    height.div_ceil(1u32 << shift)
}

impl Drop for SharedBuffer {
    fn drop(&mut self) {
        debug_status(in_context(self.cuda, self.ctx, || {
            both(
                // SAFETY: the context is current; the mapped buffer.
                check("freeing shared memory", unsafe {
                    (self.cuda.free)(self.ptr)
                }),
                // SAFETY: the import, once its mapping is freed, as CUDA asks.
                check("destroying shared memory", unsafe {
                    (self.cuda.destroy)(self.memory)
                }),
            )
        }));
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn picture(height: u32, layout: format::Pixel) -> frame::Video {
        let mut frame = frame::Video::empty();
        frame.set_format(format::Pixel::CUDA);
        frame.set_width(16);
        frame.set_height(height);
        // SAFETY: `frame` owns this zeroed frames context; `planes_fit` reads only its `sw_format`.
        unsafe {
            let ctx = ffi::av_buffer_allocz(std::mem::size_of::<ffi::AVHWFramesContext>());
            assert!(!ctx.is_null());
            (*((*ctx).data as *mut ffi::AVHWFramesContext)).sw_format = layout.into();
            (*frame.as_mut_ptr()).hw_frames_ctx = ctx;
        }
        frame
    }

    fn rejected(size: u64, height: u32, layout: format::Pixel, planes: &[PlaneCopy]) -> String {
        planes_fit(size, &picture(height, layout), planes)
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn a_pitch_that_wraps_does_not_fit() {
        let planes = [PlaneCopy {
            offset: 0,
            pitch: u64::MAX,
            row_bytes: 1,
            rows: 2,
        }];
        assert_eq!(
            rejected(u64::MAX, 1080, format::Pixel::NV12, &planes),
            "a plane of 2 rows of 1 bytes doesn't fit the shared buffer"
        );
    }

    #[test]
    fn an_offset_that_wraps_does_not_fit() {
        let planes = [PlaneCopy {
            offset: u64::MAX - 5,
            pitch: 10,
            row_bytes: 10,
            rows: 1,
        }];
        assert_eq!(
            rejected(u64::MAX, 1080, format::Pixel::NV12, &planes),
            "a plane of 1 rows of 10 bytes doesn't fit the shared buffer"
        );
    }

    #[test]
    fn a_wider_row_than_the_pitch_does_not_fit() {
        let planes = [PlaneCopy {
            offset: 0,
            pitch: 4,
            row_bytes: 5,
            rows: 1,
        }];
        assert_eq!(
            rejected(100, 10, format::Pixel::NV12, &planes),
            "a plane of 1 rows of 5 bytes doesn't fit the shared buffer"
        );
    }

    #[test]
    fn a_plane_one_byte_past_the_buffer_does_not_fit() {
        let planes = [PlaneCopy {
            offset: 10,
            pitch: 20,
            row_bytes: 8,
            rows: 3,
        }];
        assert_eq!(
            rejected(57, 3, format::Pixel::NV12, &planes),
            "a plane of 3 rows of 8 bytes doesn't fit the shared buffer"
        );
    }

    #[test]
    fn a_plane_that_ends_on_the_last_byte_fits() {
        let planes = [PlaneCopy {
            offset: 10,
            pitch: 20,
            row_bytes: 8,
            rows: 3,
        }];
        assert_eq!(
            planes_fit(58, &picture(3, format::Pixel::NV12), &planes).map_err(|e| e.to_string()),
            Ok(())
        );
    }

    #[test]
    fn a_chroma_plane_past_half_the_picture_is_rejected() {
        let planes = [
            PlaneCopy {
                offset: 0,
                pitch: 8,
                row_bytes: 8,
                rows: 1080,
            },
            PlaneCopy {
                offset: 8640,
                pitch: 8,
                row_bytes: 8,
                rows: 541,
            },
        ];
        assert_eq!(
            rejected(u64::MAX, 1080, format::Pixel::NV12, &planes),
            "a plane of 541 rows is past the picture's plane of 540"
        );
    }

    #[test]
    fn an_odd_picture_s_chroma_rows_round_up() {
        let planes = [
            PlaneCopy {
                offset: 0,
                pitch: 8,
                row_bytes: 8,
                rows: 1081,
            },
            PlaneCopy {
                offset: 8648,
                pitch: 8,
                row_bytes: 8,
                rows: 541,
            },
        ];
        assert_eq!(
            planes_fit(u64::MAX, &picture(1081, format::Pixel::NV12), &planes)
                .map_err(|e| e.to_string()),
            Ok(())
        );
    }

    #[test]
    fn a_4_2_2_chroma_plane_is_as_tall_as_the_picture() {
        let planes = [
            PlaneCopy {
                offset: 0,
                pitch: 8,
                row_bytes: 8,
                rows: 720,
            },
            PlaneCopy {
                offset: 5760,
                pitch: 8,
                row_bytes: 8,
                rows: 720,
            },
        ];
        assert_eq!(
            planes_fit(u64::MAX, &picture(720, format::Pixel::P210LE), &planes)
                .map_err(|e| e.to_string()),
            Ok(())
        );
    }

    thread_local! {
        static POPS: Cell<u32> = const { Cell::new(0) };
    }

    unsafe extern "C" fn count_pop(_: *mut CuContext) -> CuResult {
        POPS.with(|n| n.set(n.get() + 1));
        0
    }

    #[test]
    fn a_panic_pops_the_context() {
        POPS.with(|n| n.set(0));
        let caught = std::panic::catch_unwind(|| {
            let _current = PushedContext { pop: count_pop };
            panic!("while the context is current");
        });
        let message = caught.expect_err("the work panics");
        assert_eq!(
            message.downcast_ref::<&str>().copied(),
            Some("while the context is current")
        );
        assert_eq!(POPS.with(Cell::get), 1);
    }

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

    #[test]
    fn a_later_failure_is_the_error() {
        let next = Err(Error::Invalid(
            "CUDA: leaving the context failed (error 1)".into(),
        ));
        assert_eq!(
            both(Ok(1), next).unwrap_err().to_string(),
            "CUDA: leaving the context failed (error 1)"
        );
    }

    #[test]
    fn an_earlier_failure_is_the_error() {
        let work: Result<u8> = Err(Error::Invalid(
            "CUDA: copying a plane failed (error 2)".into(),
        ));
        assert_eq!(
            both(work, Ok(())).unwrap_err().to_string(),
            "CUDA: copying a plane failed (error 2)"
        );
    }

    #[test]
    fn two_failures_are_both_the_error() {
        let work: Result<u8> = Err(Error::Invalid(
            "CUDA: mapping shared memory failed (error 2)".into(),
        ));
        let next = Err(Error::Invalid(
            "CUDA: destroying shared memory failed (error 3)".into(),
        ));
        assert_eq!(
            both(work, next).unwrap_err().to_string(),
            "CUDA: mapping shared memory failed (error 2); CUDA: destroying shared memory failed (error 3)"
        );
    }

    #[test]
    fn a_negative_source_pitch_doesnt_fit() {
        let err = source_pitch(-1).unwrap_err();
        assert_eq!(err.to_string(), "a picture stride of -1 doesn't fit");
    }

    #[test]
    fn the_largest_source_pitch_fits() {
        assert_eq!(source_pitch(i32::MAX).unwrap(), 2_147_483_647);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn a_copy_length_holds_every_u64() {
        assert_eq!(copy_len(u64::MAX).unwrap(), usize::MAX);
    }

    #[cfg(target_pointer_width = "32")]
    #[test]
    fn a_copy_length_past_usize_doesnt_fit() {
        let err = copy_len(4_294_967_296).unwrap_err();
        assert_eq!(err.to_string(), "4294967296 doesn't fit a copy");
    }
}
