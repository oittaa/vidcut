//! Where a job's frames go between the decoder and their destination.
//! [`plan`] decides it for each frame, a pure function of facts about the
//! frame ([`Facts`]). The backends do the work and say what they can do
//! (`can_fit`): [`crate::cuda`] for CUDA frames, [`crate::hw::vaapi`] for
//! VA-API's, `hw::d3d11` for Direct3D 11's and `export::normalize` for the
//! CPU.

use crate::ffmpeg::{format, frame};

/// Where a frame's picture is: in memory, or in a GPU's.
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
    /// Where `f` is, by its pixel format.
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

/// Where a job's pictures go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dest {
    /// Pictures in memory: the CPU's encoders, tiles, renders, thumbnails.
    Memory,
    /// An encoder that takes frames on `place` as they are.
    Encoder { place: Place },
    /// Frames as decoded, for a display. With `keep`, it shows GPU frames
    /// from there.
    Display { keep: bool },
}

/// What fitting a frame into the output takes, for a backend's `can_fit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ops {
    /// The clockwise turn that shows the frame upright: 0, 90, 180 or 270.
    pub rotation: u16,
    /// The frame's layout (`hw::sw_format` for a GPU frame).
    pub layout: format::Pixel,
    /// The output's layout.
    pub out_layout: format::Pixel,
    /// An RGB picture (an image), not YUV.
    pub rgb: bool,
    /// The output keeps the frame's colors: no matrix, range or transfer
    /// to change (`color::kept`).
    pub colors_kept: bool,
    /// The encoder's own frames are there to fit into. AMF takes
    /// Direct3D 11's frames of those alone.
    pub into: bool,
}

/// What the planner knows of one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    pub place: Place,
    /// Its GPU can make the output picture from it for an encoder there
    /// (`hw::can_fit`).
    pub fits: bool,
    /// Fitting its source's frames on its GPU failed before.
    pub fit_failed: bool,
    /// The output is smaller than the frame each way, so its GPU would
    /// shrink it before it comes down.
    pub shrinks: bool,
    /// Its layout is one a display takes (`normalize::displayable`).
    pub displayable: bool,
    /// Keeping the job's frames on the GPU failed before.
    pub keep_failed: bool,
}

/// How a frame's picture is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// On the GPU where the frame is, handed on there.
    Gpu,
    /// Shrunk on the GPU, copied down, finished on the CPU.
    Shrunk,
    /// Copied down whole, made on the CPU.
    Whole,
    /// Made on the CPU from a frame in memory.
    Cpu,
    /// Kept on the GPU as decoded, for a display.
    Kept,
}

/// The route for a frame `facts` describe, to `dest`. A route that works
/// on the GPU wins.
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
        Dest::Encoder { place } if place == facts.place && facts.fits && !facts.fit_failed => {
            Route::Gpu
        }
        _ if facts.shrinks => Route::Shrunk,
        _ => Route::Whole,
    }
}

/// How a job's frames went, for people (`made_with`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Went {
    /// They stayed on the GPU from the decoder to the encoder.
    Stayed,
    /// They came down from the GPU at `size`. With `whole`, that is their
    /// own size.
    Down { size: (u32, u32), whole: bool },
    /// The CPU decoded them.
    Cpu,
}

impl Went {
    /// `frames stayed there`, `frames came down at 960×540` or `frames
    /// came down whole`. `None` for the CPU's.
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
            place: Place::Cuda,
            fits: true,
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
            fit_failed: false,
            shrinks: true,
            displayable: false,
            keep_failed: false,
        };
        assert_eq!(plan(&facts, Dest::Display { keep: true }), Route::Whole);
    }
}
