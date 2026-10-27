use std::ffi::{c_int, c_void};

use crate::error::{Error, Result};
use crate::ffmpeg::{self, ffi, frame};
use crate::route::Ops;

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

pub(crate) fn can_fit(ops: &Ops) -> bool {
    ops.colors_kept
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
    transfer: Option<(
        crate::frame_processor::Transfer,
        crate::frame_processor::Frames,
    )>,
    fitted: (u32, u32),
    out: (u32, u32),
    layout: ffmpeg::format::Pixel,
}

#[cfg(target_os = "macos")]
impl Fit {
    pub(super) fn new(
        f: &frame::Video,
        frames: *mut ffi::AVBufferRef,
        rotation: u16,
        (fitted, out): ((u32, u32), (u32, u32)),
        layout: ffmpeg::format::Pixel,
    ) -> Result<Self> {
        let pixel = ffi::AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX;
        let turn = turn_spec(rotation)
            .map(|spec| super::gpu_graph(f, frames, pixel, spec))
            .transpose()?;
        let upright = if rotation % 180 == 90 {
            (f.height(), f.width())
        } else {
            (f.width(), f.height())
        };
        let lanczos = (upright != fitted || fitted != out)
            .then(crate::lanczos::Lanczos::new)
            .transpose()?;
        let transfer = match super::sw_format(f) == layout {
            true => None,
            false => Some((
                crate::frame_processor::Transfer::new(FITTING)?,
                Default::default(),
            )),
        };
        Ok(Self {
            turn,
            lanczos,
            transfer,
            fitted,
            out,
            layout,
        })
    }

    pub(super) fn run(&mut self, f: &frame::Video) -> Result<frame::Video> {
        let turned = match &mut self.turn {
            Some(graph) => super::run_graph(graph, f, FITTING)?,
            None => crate::export::video::frame_ref(f)?,
        };
        let sized = match &mut self.lanczos {
            Some(lanczos) => {
                let at = crate::lanczos::centered(&turned, self.out, self.fitted);
                lanczos.place(&turned, self.out, (at, self.fitted))?
            }
            None => turned,
        };
        match &mut self.transfer {
            Some((transfer, made)) => {
                let from = crate::frame_processor::pixels(&sized, FITTING)?;
                let shape = (self.out, self.layout.into());
                made.made_as(transfer, from, &sized, shape, FITTING)
            }
            None => Ok(sized),
        }
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
        _sizes: ((u32, u32), (u32, u32)),
        _layout: ffmpeg::format::Pixel,
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
    use crate::ffmpeg::format;

    fn kept() -> Ops {
        Ops {
            rotation: 0,
            layout: format::Pixel::NV12,
            out_layout: format::Pixel::NV12,
            rgb: false,
            colors_kept: true,
            pads: false,
            into: false,
        }
    }

    #[test]
    fn frames_that_fill_the_output_in_its_layout_and_colors_fit() {
        assert!(can_fit(&kept()));
    }

    #[test]
    fn frames_placed_on_black_fit() {
        assert!(can_fit(&Ops {
            pads: true,
            ..kept()
        }));
    }

    #[test]
    fn frames_whose_colors_change_dont_fit() {
        assert!(!can_fit(&Ops {
            colors_kept: false,
            ..kept()
        }));
    }

    #[test]
    fn fitting_an_upright_frame_turns_nothing() {
        assert_eq!(turn_spec(0), None);
    }

    #[test]
    fn fitting_a_frame_turned_a_quarter_turns_it_first() {
        assert_eq!(turn_spec(90), Some("transpose_vt=dir=clock"));
    }
}
