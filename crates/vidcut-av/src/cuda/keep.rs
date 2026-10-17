use std::ffi::c_void;

use super::share::{CuDevicePtr, Cuda, check, cuda_context, in_context};
use crate::error::{Error, Result};
use crate::ffmpeg::{self, ffi, format, frame};

const MEMORY_POOLS_SUPPORTED: i32 = 115;

type Like = (usize, format::Pixel, i32, i32);

pub(crate) struct Keeper {
    frames: *mut ffi::AVBufferRef,
    like: Like,
}

// SAFETY: a frames context is reference-counted and may be used on any thread.
unsafe impl Send for Keeper {}

impl Drop for Keeper {
    fn drop(&mut self) {
        // SAFETY: its own reference.
        unsafe { ffi::av_buffer_unref(&mut self.frames) };
    }
}

fn frames_of(f: &frame::Video) -> Result<*const ffi::AVHWFramesContext> {
    // SAFETY: reads the frame's own field, and a live buffer reference.
    unsafe {
        let frames = (*f.as_ptr()).hw_frames_ctx;
        if frames.is_null() {
            return Err(Error::Invalid("a CUDA frame without its frames".into()));
        }
        Ok((*frames).data as *const ffi::AVHWFramesContext)
    }
}

unsafe fn like(frames: *const ffi::AVHWFramesContext) -> Like {
    // SAFETY: as the caller says.
    unsafe {
        (
            (*(*frames).device_ref).data as usize,
            format::Pixel::from((*frames).sw_format),
            (*frames).width,
            (*frames).height,
        )
    }
}

impl Keeper {
    pub(crate) fn new(f: &frame::Video) -> Result<Option<Self>> {
        let cuda = Cuda::get()?;
        let source = frames_of(f)?;
        // SAFETY: the frame's own frames context.
        let (device, like) = unsafe { ((*source).device_ref, like(source)) };
        if cuda.alloc_async.is_none() || cuda.free_async.is_none() {
            return Ok(None);
        }
        // SAFETY: a CUDA frames context's device.
        let (ctx, _) = unsafe { cuda_context(device) };
        let pools = in_context(cuda, ctx, || {
            let mut ordinal = 0;
            // SAFETY: the context is current; the ordinal goes into `ordinal`.
            check("finding the device", unsafe {
                (cuda.ctx_get_device)(&mut ordinal)
            })?;
            let mut pools = 0;
            // SAFETY: the ordinal just read; the answer goes into `pools`.
            check("asking for memory pools", unsafe {
                (cuda.device_get_attribute)(&mut pools, MEMORY_POOLS_SUPPORTED, ordinal)
            })?;
            Ok(pools)
        })?;
        if pools == 0 {
            return Ok(None);
        }
        // SAFETY: `device` is `f`'s live CUDA device; `frames` is set before init, freed on failure.
        unsafe {
            let mut frames = ffi::av_hwframe_ctx_alloc(device);
            if frames.is_null() {
                return Err(Error::Invalid("no memory for frames".into()));
            }
            let fc = (*frames).data as *mut ffi::AVHWFramesContext;
            (*fc).format = ffi::AVPixelFormat::AV_PIX_FMT_CUDA;
            (*fc).sw_format = (*source).sw_format;
            (*fc).width = (*source).width;
            (*fc).height = (*source).height;
            (*fc).initial_pool_size = 0;
            let ret = ffi::av_hwframe_ctx_init(frames);
            if ret < 0 {
                ffi::av_buffer_unref(&mut frames);
                return Err(Error::Ffmpeg {
                    context: "making CUDA frames to keep".into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
            Ok(Some(Self { frames, like }))
        }
    }

    pub(crate) fn is_for(&self, f: &frame::Video) -> bool {
        // SAFETY: the frame's own frames context.
        frames_of(f).is_ok_and(|source| unsafe { like(source) } == self.like)
    }

    pub(crate) fn copy(&self, f: &frame::Video) -> Result<frame::Video> {
        let cuda = Cuda::get()?;
        let (Some(alloc), Some(free)) = (cuda.alloc_async, cuda.free_async) else {
            return Err(Error::Invalid(
                "CUDA has no stream-ordered allocator".into(),
            ));
        };
        let layout = self.like.1;
        let height = f.height() as i32;
        // SAFETY: `f` is a CUDA frame: its planes are device pointers, rows `linesize` apart.
        let (data, linesize) = unsafe {
            let raw = &*f.as_ptr();
            (raw.data, raw.linesize)
        };
        let pitches = linesize.map(|l| l as isize);
        let mut sizes = [0usize; 4];
        // SAFETY: four sizes for four pitches.
        let ret = unsafe {
            ffi::av_image_fill_plane_sizes(
                sizes.as_mut_ptr(),
                layout.into(),
                height,
                pitches.as_ptr(),
            )
        };
        if ret < 0 {
            return Err(Error::Ffmpeg {
                context: "measuring a CUDA frame's planes".into(),
                source: ffmpeg::Error::from(ret),
            });
        }
        let bytes: usize = sizes.iter().sum();
        // SAFETY: its frames context's device, a CUDA device.
        let device =
            unsafe { (*((*self.frames).data as *const ffi::AVHWFramesContext)).device_ref };
        // SAFETY: a CUDA device, as `cuda_context` asks.
        let (ctx, stream) = unsafe { cuda_context(device) };
        let ptr = in_context(cuda, ctx, || {
            let mut ptr: CuDevicePtr = 0;
            // SAFETY: the context is current; the memory goes into `ptr`.
            check("allocating a kept picture", unsafe {
                alloc(&mut ptr, bytes, stream)
            })?;
            let mut at = ptr;
            for (i, &size) in sizes.iter().enumerate().take_while(|(_, s)| **s > 0) {
                let pitch = linesize[i] as usize;
                // SAFETY: plane `i` fits `f` and `ptr`'s memory, on the stream `f` was copied on.
                let copied = unsafe {
                    cuda.copy_rows(
                        (data[i] as CuDevicePtr, pitch),
                        (at, pitch),
                        (pitch, size / pitch),
                        stream,
                    )
                };
                if let Err(e) = check("copying a kept picture", copied) {
                    // SAFETY: `ptr` is ours alone; freed on `stream`, after the copies queued on it.
                    unsafe { free(ptr, stream) };
                    return Err(e);
                }
                at += size as CuDevicePtr;
            }
            Ok(ptr)
        })?;
        // SAFETY: `ptr` is ours; `release` frees it with `held`, which keeps its context and stream.
        let buffer = unsafe {
            let held = ffi::av_buffer_ref(device);
            let buffer = if held.is_null() {
                std::ptr::null_mut()
            } else {
                ffi::av_buffer_create(ptr as *mut u8, bytes, Some(release), held.cast(), 0)
            };
            if buffer.is_null() {
                let _ = in_context(cuda, ctx, || {
                    check("freeing a kept picture", free(ptr, stream))
                });
                let mut held = held;
                ffi::av_buffer_unref(&mut held);
                return Err(Error::Invalid("no memory for a kept picture".into()));
            }
            buffer
        };
        let mut out = frame::Video::empty();
        // SAFETY: fresh `out` takes `buffer` and a frames reference; planes at `f`'s pitches in `ptr`.
        unsafe {
            let o = out.as_mut_ptr();
            (*o).format = ffi::AVPixelFormat::AV_PIX_FMT_CUDA as i32;
            (*o).width = f.width() as i32;
            (*o).height = height;
            (*o).buf[0] = buffer;
            (*o).hw_frames_ctx = ffi::av_buffer_ref(self.frames);
            (*o).linesize = linesize;
            ffi::av_image_fill_pointers(
                (*o).data.as_mut_ptr(),
                layout.into(),
                height,
                ptr as *mut u8,
                linesize.as_ptr(),
            );
        }
        Ok(out)
    }
}

unsafe extern "C" fn release(opaque: *mut c_void, data: *mut u8) {
    let mut device = opaque as *mut ffi::AVBufferRef;
    if let Ok(cuda) = Cuda::get()
        && let Some(free) = cuda.free_async
    {
        // SAFETY: a CUDA device, and memory allocated on its stream.
        let (ctx, stream) = unsafe { cuda_context(device) };
        let _ = in_context(cuda, ctx, || {
            // SAFETY: the context is current; memory allocated on `stream`.
            check("freeing a kept picture", unsafe {
                free(data as CuDevicePtr, stream)
            })
        });
    }
    // SAFETY: the reference taken for it.
    unsafe { ffi::av_buffer_unref(&mut device) };
}
