//! VideoToolbox's frames on a Mac: the filters that shrink and fit them on
//! the GPU, `scale_vt` and `transpose_vt`, which keep the frames' layout
//! and place nothing on black.

use crate::route::Ops;

/// Whether VideoToolbox fits a frame into the output as `ops` says
/// ([`super::Fitter`]): where its colors stay, its picture fills the
/// output and its layout is the output's. `scale_vt` keeps the input's
/// layout (vf_scale_vt.c), and neither filter places a picture on black.
/// `scale_vt` can convert colors, but no test compares that with the
/// CPU's math.
pub(crate) fn can_fit(ops: &Ops) -> bool {
    ops.colors_kept && !ops.pads && ops.layout == ops.out_layout
}

/// The filter that shrinks VideoToolbox's frames to `w` by `h`.
pub(super) fn shrink_spec((w, h): (u32, u32)) -> String {
    format!("scale_vt=w={w}:h={h}")
}

/// The filters that fit VideoToolbox's frames into the output: turned
/// clockwise by `rotation` and scaled to `fw` by `fh`, the output's size.
pub(super) fn fit_spec(rotation: u16, (fw, fh): (u32, u32)) -> String {
    // As the CPU's `transpose` and flips turn them.
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

    /// An upright NV12 frame into NV12 pictures in its own colors.
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
    fn ten_bit_frames_dont_fit_into_eight_bit_pictures() {
        assert!(!can_fit(&Ops {
            layout: format::Pixel::P010LE,
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
