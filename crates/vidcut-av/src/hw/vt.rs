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

#[cfg(any(target_os = "macos", test))]
fn turn_spec(rotation: u16) -> Option<&'static str> {
    match rotation {
        90 => Some("transpose_vt=dir=clock"),
        180 => Some("transpose_vt=dir=reversal"),
        270 => Some("transpose_vt=dir=cclock"),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
const FITTING: &str = "fitting a frame on the GPU";

#[cfg(target_os = "macos")]
pub(super) struct Fit {
    turn: Option<super::GpuGraph>,
    lanczos: Option<crate::lanczos::Lanczos>,
    size: (u32, u32),
}

#[cfg(target_os = "macos")]
impl Fit {
    pub(super) fn new(
        f: &frame::Video,
        frames: *mut ffi::AVBufferRef,
        rotation: u16,
        size: (u32, u32),
    ) -> Result<Self> {
        let pixel = ffi::AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX;
        let turn = turn_spec(rotation)
            .map(|spec| super::gpu_graph(f, frames, pixel, spec))
            .transpose()?;
        Ok(Self {
            turn,
            lanczos: None,
            size,
        })
    }

    pub(super) fn run(&mut self, f: &frame::Video) -> Result<frame::Video> {
        let turned = match &mut self.turn {
            Some(graph) => super::run_graph(graph, f, FITTING)?,
            None => crate::export::video::frame_ref(f)?,
        };
        Ok(crate::lanczos::sized(&mut self.lanczos, &turned, self.size)?.unwrap_or(turned))
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) struct Fit(std::convert::Infallible);

#[cfg(not(target_os = "macos"))]
impl Fit {
    pub(super) fn new(
        _f: &frame::Video,
        _frames: *mut ffi::AVBufferRef,
        _rotation: u16,
        _size: (u32, u32),
    ) -> Result<Self> {
        Err(Error::Invalid(
            "VideoToolbox frames fit on macOS only".into(),
        ))
    }

    pub(super) fn run(&mut self, _f: &frame::Video) -> Result<frame::Video> {
        match self.0 {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitting_an_upright_frame_turns_nothing() {
        assert_eq!(turn_spec(0), None);
    }

    #[test]
    fn fitting_a_frame_turned_a_quarter_turns_it_first() {
        assert_eq!(turn_spec(90), Some("transpose_vt=dir=clock"));
    }
}
