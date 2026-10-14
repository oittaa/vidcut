//! Where a job's frames go between the decoder and their destination
//! (docs/todo/gpu-frames.md): fitted on the GPU that decoded them and handed
//! on there, shrunk there and copied down, copied down whole, or made on the
//! CPU. [`plan`] decides it for each source from facts about its frames and
//! the destination alone. The backends do the work: `cuda` for NVIDIA's,
//! `hw` for VA-API's, Direct3D 11's and VideoToolbox's.

use vidcut_core::export::EncoderChoice;

use crate::ffmpeg::{format, frame};

/// Where a frame's picture is.
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
    /// Where `f` is.
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

/// Where an encoder of `kind` takes GPU frames from as they are, when it is
/// on the GPU that decoded them; `None`: it takes frames from memory.
pub fn takes(kind: EncoderChoice) -> Option<Place> {
    match kind {
        EncoderChoice::Nvenc => Some(Place::Cuda),
        EncoderChoice::Vaapi => Some(Place::Vaapi),
        EncoderChoice::Amf => Some(Place::D3d11),
        EncoderChoice::Vulkan => Some(Place::Vulkan),
        EncoderChoice::VideoToolbox => Some(Place::VideoToolbox),
        EncoderChoice::Qsv | EncoderChoice::Cpu | EncoderChoice::Auto => None,
    }
}

/// Whether an encoder of `kind` on `device` (as the encoder check names it)
/// takes `decoder`'s frames on the decoder's own device. A Mac has one
/// VideoToolbox, which names no device.
pub(crate) fn shares(
    kind: EncoderChoice,
    device: Option<&str>,
    decoder: &crate::hw::GpuDecoder,
) -> bool {
    takes(kind) == Some(decoder.kind().place())
        && (kind == EncoderChoice::VideoToolbox || device == Some(decoder.on()))
}

/// Where a job's pictures go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dest {
    /// Pictures in memory: the CPU's encoders, tiles, renders.
    Memory,
    /// An encoder that takes frames on `place`, on the GPU that decodes;
    /// pictures made on the CPU go up to it.
    Encoder { place: Place },
    /// Frames as decoded, for a display: kept on the GPU where it shows
    /// them from there (`keep`).
    Display { keep: bool },
}

/// What a source's frames are, and what they need, as its first frame says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    pub place: Place,
    /// Their layout: in memory, or under a GPU's frames (NV12, P010).
    pub layout: format::Pixel,
    /// Their clockwise turn: 0, 90, 180 or 270.
    pub rotation: u16,
    /// RGB pictures (images), not YUV.
    pub rgb: bool,
    /// Their colors are the output's: no matrix, range or transfer to change.
    pub colors_kept: bool,
    /// They are bigger than the output picture each way.
    pub shrinks: bool,
    /// Their picture is placed on black in the output: its aspect differs.
    pub pads: bool,
    /// The output's layout.
    pub out_layout: format::Pixel,
    /// Direct3D 11: the encoder's frames are there to fit into (AMF takes
    /// those alone).
    pub into: bool,
    /// Fitting this source's frames on the GPU failed already.
    pub fit_failed: bool,
}

/// How a source's frames reach the destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Fitted on the GPU where they are, and handed on there: nothing copied.
    Gpu,
    /// Shrunk on the GPU to the size they are shown at, copied down, and
    /// finished on the CPU; up to an encoder that takes GPU frames.
    Shrunk,
    /// Copied down whole and made on the CPU; up to such an encoder.
    Whole,
    /// Decoded on the CPU and made there.
    Cpu,
    /// Kept on the GPU as decoded, for a display.
    Kept,
}

/// How a source's frames went, as they did: what a job says it did. A GPU
/// that won't shrink them on its route lets them come down whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Went {
    /// Stayed on the GPU that decoded them.
    Stayed,
    /// Copied down at `size`, shrunk on the GPU first unless `whole`.
    Down { size: (u32, u32), whole: bool },
    /// Decoded on the CPU.
    Cpu,
}

impl Went {
    /// For people, after where they were decoded: `frames came down at
    /// 960×540`. `None` on the CPU.
    pub fn label(self) -> Option<String> {
        match self {
            Self::Stayed => Some("frames stayed there".into()),
            Self::Down { whole: true, .. } => Some("frames came down whole".into()),
            Self::Down {
                size: (w, h),
                whole: false,
            } => Some(format!("frames came down at {w}×{h}")),
            Self::Cpu => None,
        }
    }
}

/// The route for frames that `facts` describe, to `dest`. A route that works
/// on the GPU wins: the CPU being faster never takes frames off it.
pub fn plan(facts: &Facts, dest: Dest) -> Route {
    if facts.place == Place::Memory {
        return Route::Cpu;
    }
    match dest {
        Dest::Display { keep } if keep && displayable(facts.layout) => Route::Kept,
        Dest::Display { .. } => Route::Whole,
        Dest::Encoder { place } if !facts.fit_failed && fits(facts, place) => Route::Gpu,
        _ if facts.shrinks && shrinks(facts.place) => Route::Shrunk,
        _ => Route::Whole,
    }
}

/// Whether the GPU where `facts`' frames are fits them into the output for
/// an encoder that takes frames on `to`, on the same GPU: turns, scales,
/// converts and pads them.
fn fits(facts: &Facts, to: Place) -> bool {
    let turnable = matches!(facts.rotation, 0 | 90 | 180 | 270);
    match (facts.place, to) {
        // `cuda::CudaPictures`: libplacebo turns them and converts their
        // colors, HDR too; it takes no RGB. For a Vulkan encoder its
        // pictures stay in Vulkan.
        (Place::Cuda, Place::Cuda | Place::Vulkan) => turnable && !facts.rgb,
        // `hw::Fitter`: turned, scaled and padded, colors as they are.
        (Place::Vaapi, Place::Vaapi) => turnable && facts.colors_kept,
        // The video processor, into the encoder's frames; AMD's makes no
        // 10-bit pictures of 8-bit ones.
        (Place::D3d11, Place::D3d11) => {
            turnable
                && facts.colors_kept
                && facts.into
                && !(facts.layout == format::Pixel::NV12
                    && facts.out_layout == format::Pixel::P010LE)
        }
        // `transpose_vt` and `scale_vt`: no padding, and the layout stays
        // the frames' (vf_scale_vt.c).
        (Place::VideoToolbox, Place::VideoToolbox) => {
            turnable && facts.colors_kept && !facts.pads && facts.layout == facts.out_layout
        }
        _ => false,
    }
}

/// Whether the GPU where frames are shrinks them before they come down
/// (`hw::Shrinker`).
fn shrinks(place: Place) -> bool {
    matches!(
        place,
        Place::Cuda | Place::Vaapi | Place::D3d11 | Place::VideoToolbox
    )
}

/// The layouts a display takes: 8-bit 4:2:0 (planar, or NV12 as hardware
/// decoders give it) and P010 for more bits.
pub(crate) fn displayable(p: format::Pixel) -> bool {
    use format::Pixel as P;
    matches!(p, P::YUV420P | P::YUVJ420P | P::NV12 | P::P010LE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use format::Pixel::{NV12, P010LE, YUV420P};

    /// 4K NV12 frames from `place`, upright, in the output's colors, for a
    /// smaller 8-bit output.
    fn decoded(place: Place) -> Facts {
        Facts {
            place,
            layout: NV12,
            rotation: 0,
            rgb: false,
            colors_kept: true,
            shrinks: true,
            pads: false,
            out_layout: NV12,
            into: false,
            fit_failed: false,
        }
    }

    #[test]
    fn frames_in_memory_are_made_on_the_cpu() {
        let facts = Facts {
            layout: YUV420P,
            ..decoded(Place::Memory)
        };
        assert_eq!(
            plan(&facts, Dest::Encoder { place: Place::Cuda }),
            Route::Cpu
        );
    }

    #[test]
    fn nvdecs_frames_stay_on_the_gpu_for_nvenc() {
        let dest = Dest::Encoder { place: Place::Cuda };
        assert_eq!(plan(&decoded(Place::Cuda), dest), Route::Gpu);
    }

    #[test]
    fn nvdecs_frames_stay_on_the_gpu_for_a_vulkan_encoder() {
        let dest = Dest::Encoder {
            place: Place::Vulkan,
        };
        assert_eq!(plan(&decoded(Place::Cuda), dest), Route::Gpu);
    }

    #[test]
    fn nvdecs_frames_in_other_colors_stay_on_the_gpu_for_nvenc() {
        let facts = Facts {
            colors_kept: false,
            rotation: 90,
            ..decoded(Place::Cuda)
        };
        assert_eq!(
            plan(&facts, Dest::Encoder { place: Place::Cuda }),
            Route::Gpu
        );
    }

    #[test]
    fn vaapis_frames_stay_on_the_gpu_for_vaapi() {
        let dest = Dest::Encoder {
            place: Place::Vaapi,
        };
        assert_eq!(plan(&decoded(Place::Vaapi), dest), Route::Gpu);
    }

    #[test]
    fn vaapis_frames_in_other_colors_shrink_and_come_down() {
        let facts = Facts {
            colors_kept: false,
            ..decoded(Place::Vaapi)
        };
        let dest = Dest::Encoder {
            place: Place::Vaapi,
        };
        assert_eq!(plan(&facts, dest), Route::Shrunk);
    }

    #[test]
    fn direct3d_11s_frames_fit_into_amfs_frames() {
        let facts = Facts {
            into: true,
            ..decoded(Place::D3d11)
        };
        let dest = Dest::Encoder {
            place: Place::D3d11,
        };
        assert_eq!(plan(&facts, dest), Route::Gpu);
    }

    #[test]
    fn direct3d_11s_8_bit_frames_come_down_for_10_bit_output() {
        let facts = Facts {
            into: true,
            out_layout: P010LE,
            ..decoded(Place::D3d11)
        };
        let dest = Dest::Encoder {
            place: Place::D3d11,
        };
        assert_eq!(plan(&facts, dest), Route::Shrunk);
    }

    #[test]
    fn frames_for_an_encoder_on_another_gpu_shrink_and_come_down() {
        let dest = Dest::Encoder { place: Place::Cuda };
        assert_eq!(plan(&decoded(Place::Vaapi), dest), Route::Shrunk);
    }

    #[test]
    fn frames_after_a_failed_fit_shrink_and_come_down() {
        let facts = Facts {
            fit_failed: true,
            ..decoded(Place::Vaapi)
        };
        let dest = Dest::Encoder {
            place: Place::Vaapi,
        };
        assert_eq!(plan(&facts, dest), Route::Shrunk);
    }

    #[test]
    fn frames_no_bigger_than_the_output_come_down_whole() {
        let facts = Facts {
            shrinks: false,
            ..decoded(Place::Cuda)
        };
        assert_eq!(plan(&facts, Dest::Memory), Route::Whole);
    }

    #[test]
    fn videotoolboxs_frames_stay_on_the_gpu_for_its_encoder() {
        let dest = Dest::Encoder {
            place: Place::VideoToolbox,
        };
        assert_eq!(plan(&decoded(Place::VideoToolbox), dest), Route::Gpu);
    }

    #[test]
    fn videotoolboxs_10_bit_frames_shrink_and_come_down_for_8_bit_output() {
        let facts = Facts {
            layout: P010LE,
            ..decoded(Place::VideoToolbox)
        };
        let dest = Dest::Encoder {
            place: Place::VideoToolbox,
        };
        assert_eq!(plan(&facts, dest), Route::Shrunk);
    }

    #[test]
    fn videotoolboxs_frames_placed_on_black_shrink_and_come_down() {
        let facts = Facts {
            pads: true,
            ..decoded(Place::VideoToolbox)
        };
        let dest = Dest::Encoder {
            place: Place::VideoToolbox,
        };
        assert_eq!(plan(&facts, dest), Route::Shrunk);
    }

    #[test]
    fn videotoolboxs_frames_shrink_before_they_come_down() {
        let dest = Dest::Memory;
        assert_eq!(plan(&decoded(Place::VideoToolbox), dest), Route::Shrunk);
    }

    #[test]
    fn a_display_keeps_nv12_frames_on_the_gpu() {
        let dest = Dest::Display { keep: true };
        assert_eq!(plan(&decoded(Place::D3d11), dest), Route::Kept);
    }

    #[test]
    fn frames_shrunk_on_the_gpu_say_the_size_they_came_down_at() {
        let went = Went::Down {
            size: (960, 540),
            whole: false,
        };
        assert_eq!(went.label().as_deref(), Some("frames came down at 960×540"));
    }

    #[test]
    fn a_display_takes_other_layouts_down_whole() {
        let facts = Facts {
            layout: format::Pixel::YUV422P10LE,
            ..decoded(Place::VideoToolbox)
        };
        assert_eq!(plan(&facts, Dest::Display { keep: true }), Route::Whole);
    }
}
