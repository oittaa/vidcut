use std::ffi::{c_int, c_void};

use crate::error::{Error, Result};
use crate::ffmpeg::{self, ffi, frame};

pub(super) fn mapped(src: &frame::Video) -> Result<frame::Video> {
    let mut map = frame::Video::empty();
    // SAFETY: `map` is a fresh frame, mapped from `src` in its own layout, unmapped when it drops.
    let ret = unsafe {
        ffi::av_hwframe_map(
            map.as_mut_ptr(),
            src.as_ptr(),
            ffi::AV_HWFRAME_MAP_READ as c_int,
        )
    };
    if ret < 0 {
        return Err(Error::Ffmpeg {
            context: "reading a frame in GPU memory".into(),
            source: ffmpeg::Error::from(ret),
        });
    }
    // SAFETY: `map`'s own buffer reference moves into a read-only buffer that frees it.
    unsafe {
        let raw = map.as_mut_ptr();
        let held = (*raw).buf[0];
        let read_only = ffi::av_buffer_create(
            held.cast(),
            0,
            Some(unmap),
            held.cast(),
            ffi::AV_BUFFER_FLAG_READONLY,
        );
        if read_only.is_null() {
            return Err(Error::Invalid("no memory for a frame".into()));
        }
        (*raw).buf[0] = read_only;
    }
    Ok(map)
}

unsafe extern "C" fn unmap(held: *mut c_void, _data: *mut u8) {
    let mut held = held.cast::<ffi::AVBufferRef>();
    // SAFETY: the map's buffer reference that `mapped` moved in, freed once.
    unsafe { ffi::av_buffer_unref(&mut held) };
}
