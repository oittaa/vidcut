use std::ffi::c_void;

use crate::error::{Error, Result};
use crate::ffmpeg::{ffi, frame};

fn opaque_of(mark: &'static u8) -> *mut c_void {
    (mark as *const u8).cast_mut().cast()
}

unsafe extern "C" fn let_go<T>(_: *mut c_void, data: *mut u8) {
    // SAFETY: `data` is the box that `carry` made, freed once by its buffer.
    drop(unsafe { Box::from_raw(data.cast::<T>()) });
}

pub(crate) fn carry<T: Send>(f: &mut frame::Video, value: T, mark: &'static u8) -> Result<()> {
    let boxed = Box::into_raw(Box::new(value));
    // SAFETY: a buffer that frees the box once becomes the opaque buffer of `f`.
    unsafe {
        let opaque = ffi::av_buffer_create(
            boxed.cast(),
            std::mem::size_of::<T>(),
            Some(let_go::<T>),
            opaque_of(mark),
            ffi::AV_BUFFER_FLAG_READONLY,
        );
        if opaque.is_null() {
            drop(Box::from_raw(boxed));
            return Err(Error::Invalid("no memory for a picture on the GPU".into()));
        }
        let raw = f.as_mut_ptr();
        ffi::av_buffer_unref(&mut (*raw).opaque_ref);
        (*raw).opaque_ref = opaque;
    }
    Ok(())
}

pub(crate) fn carried<'a, T>(f: &'a frame::Video, mark: &'static u8) -> Option<&'a T> {
    // SAFETY: the frame's own reference; one with `mark` holds a `T` while the frame lives.
    unsafe {
        let opaque = (*f.as_ptr()).opaque_ref;
        if opaque.is_null() || ffi::av_buffer_get_opaque(opaque) != opaque_of(mark) {
            return None;
        }
        Some(&*(*opaque).data.cast::<T>())
    }
}

pub(crate) fn let_go_of(f: &mut frame::Video) {
    // SAFETY: the frame's own reference, or null.
    unsafe { ffi::av_buffer_unref(&mut (*f.as_mut_ptr()).opaque_ref) };
}
