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
    /// Direct3D 11: libplacebo can fit them on the GPU's Vulkan device,
    /// colors and HDR too, where its video processor can't.
    pub placebo: bool,
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
        // The video processor, or libplacebo on Vulkan where it can't.
        (Place::D3d11, Place::D3d11) => {
            processor_fits(facts) || (facts.placebo && facts.into && turnable && !facts.rgb)
        }
        // `transpose_vt` and `scale_vt`: no padding, and the layout stays
        // the frames' (vf_scale_vt.c).
        (Place::VideoToolbox, Place::VideoToolbox) => {
            turnable && facts.colors_kept && !facts.pads && facts.layout == facts.out_layout
        }
        _ => false,
    }
}

/// Whether Direct3D 11's video processor fits `facts`' frames into the
/// encoder's: turned, scaled and padded, colors as they are; AMD's makes no
/// 10-bit pictures of 8-bit ones.
pub(crate) fn processor_fits(facts: &Facts) -> bool {
    matches!(facts.rotation, 0 | 90 | 180 | 270)
        && facts.colors_kept
        && facts.into
        && !(facts.layout == format::Pixel::NV12 && facts.out_layout == format::Pixel::P010LE)
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
