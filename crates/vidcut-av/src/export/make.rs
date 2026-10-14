//! Output pictures made from decoded frames along the route the planner
//! gives each (`route::plan`): the part of the engine every job shares.

use crate::route::Place;

/// What a caller makes of the frames
/// ([`VideoFrames::output`](crate::export::video::VideoFrames::output)).
pub(crate) enum Output {
    /// Fitted into the output on the CPU, the default. Frames a GPU decoded
    /// come down shrunk there first.
    Fitted,
    /// Each frame as decoded: its own size and colors, upright only after
    /// [`rotation`](crate::export::video::VideoFrames::rotation), in a
    /// layout a display takes (see `Relayout`). Black is 8-bit 4:2:0. With
    /// `keep_on_gpu`, the GPU's frames stay in its memory in NV12 or P010,
    /// for a display that shows them from there.
    Decoded { keep_on_gpu: bool },
    /// Fitted for an encoder on the GPU's device, whose frames are on
    /// `place`: the frames the planner leaves there ([`plan`]) are fitted
    /// there, Direct3D 11's into `into`, the encoder's frames. The rest are
    /// fitted on the CPU.
    FittedOnGpu {
        place: Place,
        into: Option<crate::hw::Frames>,
    },
    /// For NVENC, in CUDA memory from `frames`, made for the output's size
    /// and format (NV12 or P010): pieces decoded on NVDEC are scaled there,
    /// the rest and black are made on the CPU and go up.
    Cuda {
        device: crate::cuda::CudaDevice,
        frames: crate::cuda::CudaFrames,
    },
}
