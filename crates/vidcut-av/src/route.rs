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
pub struct Facts {
    pub place: Place,
    pub keep: bool,
    pub displayable: bool,
    pub keep_failed: bool,
}

impl Facts {
    pub fn of(f: &frame::Video, keep: bool) -> Self {
        Self {
            place: Place::of(f),
            keep,
            displayable: crate::export::normalize::displayable(crate::hw::sw_format(f)),
            keep_failed: false,
        }
    }

    pub fn deinterlacing(f: &frame::Video, gpu_failed: bool) -> Self {
        let place = Place::of(f);
        Self {
            place,
            keep: !gpu_failed
                && matches!(place, Place::Cuda | Place::Vulkan)
                && crate::hw::deinterlace::gives_bwdifs(f),
            displayable: true,
            keep_failed: gpu_failed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Whole,
    Cpu,
    Kept,
}

pub fn plan(facts: &Facts) -> Route {
    if facts.place == Place::Memory {
        Route::Cpu
    } else if facts.keep && !facts.keep_failed && facts.displayable {
        Route::Kept
    } else {
        Route::Whole
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Went {
    Stayed,
    Down,
    Cpu,
}

impl Went {
    pub fn label(self) -> Option<String> {
        match self {
            Went::Stayed => Some("frames stayed there".into()),
            Went::Down => Some("frames came down whole".into()),
            Went::Cpu => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(place: Place) -> Facts {
        Facts {
            place,
            keep: true,
            displayable: true,
            keep_failed: false,
        }
    }

    #[test]
    fn a_frame_in_memory_is_made_on_the_cpu() {
        assert_eq!(plan(&on(Place::Memory)), Route::Cpu);
    }

    #[test]
    fn a_display_that_shows_gpu_frames_keeps_them() {
        assert_eq!(plan(&on(Place::D3d11)), Route::Kept);
    }

    #[test]
    fn frames_that_failed_to_keep_come_down_whole() {
        let facts = Facts {
            keep_failed: true,
            ..on(Place::D3d11)
        };
        assert_eq!(plan(&facts), Route::Whole);
    }

    #[test]
    fn a_display_that_shows_no_gpu_frames_gets_them_whole() {
        let facts = Facts {
            keep: false,
            ..on(Place::Cuda)
        };
        assert_eq!(plan(&facts), Route::Whole);
    }

    #[test]
    fn a_layout_no_display_takes_comes_down_whole() {
        let facts = Facts {
            displayable: false,
            ..on(Place::Vaapi)
        };
        assert_eq!(plan(&facts), Route::Whole);
    }
}
