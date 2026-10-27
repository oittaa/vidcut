use vidcut_core::model::Hdr;

use crate::ffmpeg::{format, frame};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Memory,
    Cuda,
    Vaapi,
    D3d11,
    VideoToolbox,
    Vulkan,
}

impl Place {
    pub fn of(f: &frame::Video) -> Self {
        match f.format() {
            format::Pixel::CUDA => Self::Cuda,
            format::Pixel::VAAPI => Self::Vaapi,
            format::Pixel::D3D11 => Self::D3d11,
            format::Pixel::VIDEOTOOLBOX => Self::VideoToolbox,
            format::Pixel::VULKAN => Self::Vulkan,
            _ => Self::Memory,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dest {
    Memory,
    Encoder { place: Place },
    Display { keep: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ops {
    pub rotation: u16,
    pub layout: format::Pixel,
    pub out_layout: format::Pixel,
    pub rgb: bool,
    pub colors_kept: bool,
    pub pads: bool,
    pub into: bool,
}

impl Ops {
    pub fn of(
        f: &frame::Video,
        rotation: u16,
        out: (u32, u32),
        out_layout: format::Pixel,
        target: Option<Hdr>,
        into: bool,
    ) -> Self {
        let colors = crate::color::Source::of(f);
        let (w, h) = crate::export::normalize::shown_size(f, rotation, out);
        let fitted = if rotation % 180 == 90 { (h, w) } else { (w, h) };
        Self {
            rotation,
            layout: crate::hw::sw_format(f),
            out_layout,
            rgb: colors.rgb(),
            colors_kept: crate::color::kept(colors, target),
            pads: fitted != out,
            into,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    pub place: Place,
    pub fits: bool,
    pub reaches: bool,
    pub fit_failed: bool,
    pub shrinks: bool,
    pub displayable: bool,
    pub keep_failed: bool,
}

impl Facts {
    pub fn of(f: &frame::Video, ops: &Ops, out: (u32, u32), dest: Dest) -> Self {
        let place = Place::of(f);
        let (w, h) = crate::export::normalize::shown_size(f, ops.rotation, out);
        let reaches = match dest {
            Dest::Encoder { place: to } => crate::hw::reaches(place, to),
            _ => false,
        };
        Self {
            place,
            fits: crate::hw::can_fit(place, ops),
            reaches,
            fit_failed: false,
            shrinks: w < f.width() && h < f.height(),
            displayable: crate::export::normalize::displayable(ops.layout),
            keep_failed: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Gpu,
    Shrunk,
    Whole,
    Cpu,
    Kept,
}

pub fn plan(facts: &Facts, dest: Dest) -> Route {
    if facts.place == Place::Memory {
        return Route::Cpu;
    }
    match dest {
        Dest::Display { keep } => {
            if keep && !facts.keep_failed && facts.displayable {
                Route::Kept
            } else {
                Route::Whole
            }
        }
        Dest::Encoder { place }
            if (place == facts.place || facts.reaches) && facts.fits && !facts.fit_failed =>
        {
            Route::Gpu
        }
        _ if facts.shrinks => Route::Shrunk,
        _ => Route::Whole,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Went {
    Stayed,
    Down { size: (u32, u32), whole: bool },
    Cpu,
}

impl Went {
    pub fn label(self) -> Option<String> {
        match self {
            Went::Stayed => Some("frames stayed there".into()),
            Went::Down { whole: true, .. } => Some("frames came down whole".into()),
            Went::Down { size: (w, h), .. } => Some(format!("frames came down at {w}×{h}")),
            Went::Cpu => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_in_memory_is_made_on_the_cpu() {
        let facts = Facts {
            place: Place::Memory,
            fits: false,
            reaches: false,
            fit_failed: false,
            shrinks: false,
            displayable: true,
            keep_failed: false,
        };
        let dest = Dest::Encoder { place: Place::Cuda };
        assert_eq!(plan(&facts, dest), Route::Cpu);
    }

    #[test]
    fn a_frame_its_gpu_fits_stays_there_for_the_encoder() {
        let facts = Facts {
            place: Place::Cuda,
            fits: true,
            reaches: false,
            fit_failed: false,
            shrinks: true,
            displayable: true,
            keep_failed: false,
        };
        let dest = Dest::Encoder { place: Place::Cuda };
        assert_eq!(plan(&facts, dest), Route::Gpu);
    }

    #[test]
    fn a_frame_whose_fitting_failed_comes_down_shrunk() {
        let facts = Facts {
            place: Place::Cuda,
            fits: true,
            reaches: false,
            fit_failed: true,
            shrinks: true,
            displayable: true,
            keep_failed: false,
        };
        let dest = Dest::Encoder { place: Place::Cuda };
        assert_eq!(plan(&facts, dest), Route::Shrunk);
    }

    #[test]
    fn a_frame_its_gpu_cant_fit_comes_down_whole_when_it_isnt_shrunk() {
        let facts = Facts {
            place: Place::Vaapi,
            fits: false,
            reaches: false,
            fit_failed: false,
            shrinks: false,
            displayable: true,
            keep_failed: false,
        };
        let dest = Dest::Encoder {
            place: Place::Vaapi,
        };
        assert_eq!(plan(&facts, dest), Route::Whole);
    }

    #[test]
    fn a_gpu_frame_for_memory_comes_down_shrunk() {
        let facts = Facts {
            place: Place::D3d11,
            fits: false,
            reaches: false,
            fit_failed: false,
            shrinks: true,
            displayable: true,
            keep_failed: false,
        };
        assert_eq!(plan(&facts, Dest::Memory), Route::Shrunk);
    }

    #[test]
    fn a_gpu_frame_no_smaller_in_the_output_comes_down_whole() {
        let facts = Facts {
            place: Place::D3d11,
            fits: false,
            reaches: false,
            fit_failed: false,
            shrinks: false,
            displayable: true,
            keep_failed: false,
        };
        assert_eq!(plan(&facts, Dest::Memory), Route::Whole);
    }

    #[test]
    fn an_encoder_on_another_place_never_takes_a_frame_as_it_is() {
        let facts = Facts {
            place: Place::Vaapi,
            fits: true,
            reaches: false,
            fit_failed: false,
            shrinks: true,
            displayable: true,
            keep_failed: false,
        };
        let dest = Dest::Encoder {
            place: Place::Vulkan,
        };
        assert_eq!(plan(&facts, dest), Route::Shrunk);
    }

    #[test]
    fn a_frame_that_reaches_the_encoders_place_stays_on_the_gpu() {
        let facts = Facts {
            place: Place::Cuda,
            fits: true,
            reaches: true,
            fit_failed: false,
            shrinks: true,
            displayable: true,
            keep_failed: false,
        };
        let dest = Dest::Encoder {
            place: Place::Vulkan,
        };
        assert_eq!(plan(&facts, dest), Route::Gpu);
    }

    #[test]
    fn a_frame_that_reaches_the_encoders_place_but_doesnt_fit_comes_down_shrunk() {
        let facts = Facts {
            place: Place::Cuda,
            fits: false,
            reaches: true,
            fit_failed: false,
            shrinks: true,
            displayable: true,
            keep_failed: false,
        };
        let dest = Dest::Encoder {
            place: Place::Vulkan,
        };
        assert_eq!(plan(&facts, dest), Route::Shrunk);
    }

    #[test]
    fn a_display_that_shows_gpu_frames_keeps_them() {
        let facts = Facts {
            place: Place::D3d11,
            fits: false,
            reaches: false,
            fit_failed: false,
            shrinks: true,
            displayable: true,
            keep_failed: false,
        };
        assert_eq!(plan(&facts, Dest::Display { keep: true }), Route::Kept);
    }

    #[test]
    fn frames_that_failed_to_keep_come_down_whole() {
        let facts = Facts {
            place: Place::D3d11,
            fits: false,
            reaches: false,
            fit_failed: false,
            shrinks: true,
            displayable: true,
            keep_failed: true,
        };
        assert_eq!(plan(&facts, Dest::Display { keep: true }), Route::Whole);
    }

    #[test]
    fn a_display_that_shows_no_gpu_frames_gets_them_whole() {
        let facts = Facts {
            place: Place::Cuda,
            fits: true,
            reaches: false,
            fit_failed: false,
            shrinks: true,
            displayable: true,
            keep_failed: false,
        };
        assert_eq!(plan(&facts, Dest::Display { keep: false }), Route::Whole);
    }

    #[test]
    fn a_layout_no_display_takes_comes_down_whole() {
        let facts = Facts {
            place: Place::Vaapi,
            fits: false,
            reaches: false,
            fit_failed: false,
            shrinks: true,
            displayable: false,
            keep_failed: false,
        };
        assert_eq!(plan(&facts, Dest::Display { keep: true }), Route::Whole);
    }

    #[test]
    fn a_picture_of_another_aspect_is_placed_on_black() {
        let f = frame::Video::new(format::Pixel::YUV420P, 1920, 800);
        let ops = Ops::of(&f, 0, (1920, 1080), format::Pixel::NV12, None, false);
        assert!(ops.pads);
    }

    #[test]
    fn a_picture_of_the_outputs_aspect_isnt_placed_on_black() {
        let f = frame::Video::new(format::Pixel::YUV420P, 1920, 1080);
        let ops = Ops::of(&f, 0, (1920, 1080), format::Pixel::NV12, None, false);
        assert!(!ops.pads);
    }

    #[test]
    fn a_portrait_picture_turned_a_quarter_fills_a_landscape_output() {
        let f = frame::Video::new(format::Pixel::YUV420P, 1080, 1920);
        let ops = Ops::of(&f, 90, (1920, 1080), format::Pixel::NV12, None, false);
        assert!(!ops.pads);
    }
}
