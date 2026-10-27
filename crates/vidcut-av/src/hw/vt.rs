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
    ops.colors_kept && !ops.pads && (ops.layout == ops.out_layout || ops.rotation == 0)
}

#[cfg(target_os = "macos")]
pub(super) fn transfer(
    from: ffmpeg::format::Pixel,
    to: ffmpeg::format::Pixel,
    rotation: u16,
) -> Result<super::Fitting> {
    if rotation != 0 {
        return Err(Error::Invalid(format!(
            "VideoToolbox doesn't change turned {from:?} frames into {to:?}"
        )));
    }
    let what = "fitting a frame on the GPU";
    Ok(super::Fitting::Transfer(
        crate::frame_processor::Transfer::new(what)?,
        crate::frame_processor::Frames::default(),
    ))
}

#[cfg(not(target_os = "macos"))]
pub(super) fn transfer(
    from: ffmpeg::format::Pixel,
    to: ffmpeg::format::Pixel,
    _rotation: u16,
) -> Result<super::Fitting> {
    Err(Error::Invalid(format!(
        "VideoToolbox doesn't change {from:?} frames into {to:?}"
    )))
}

pub(super) fn shrink_spec((w, h): (u32, u32)) -> String {
    format!("scale_vt=w={w}:h={h}")
}

pub(super) fn fit_spec(rotation: u16, (fw, fh): (u32, u32)) -> String {
    let turn = match rotation {
        90 => "transpose_vt=dir=clock,",
        180 => "transpose_vt=dir=reversal,",
        270 => "transpose_vt=dir=cclock,",
        _ => "",
    };
    format!("{turn}scale_vt=w={fw}:h={fh}")
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
    fn frames_placed_on_black_dont_fit() {
        assert!(!can_fit(&Ops {
            pads: true,
            ..kept()
        }));
    }

    #[test]
    fn turned_frames_in_another_layout_dont_fit() {
        assert!(!can_fit(&Ops {
            layout: format::Pixel::P010LE,
            rotation: 90,
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
    fn shrinking_scales_to_the_size() {
        assert_eq!(shrink_spec((960, 540)), "scale_vt=w=960:h=540");
    }

    #[test]
    fn fitting_an_upright_frame_only_scales_it() {
        assert_eq!(fit_spec(0, (1920, 1080)), "scale_vt=w=1920:h=1080");
    }

    #[test]
    fn fitting_a_frame_turned_a_quarter_turns_it_first() {
        assert_eq!(
            fit_spec(90, (608, 1080)),
            "transpose_vt=dir=clock,scale_vt=w=608:h=1080"
        );
    }
}
