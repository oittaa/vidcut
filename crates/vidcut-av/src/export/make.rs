//! Output pictures made from decoded frames along the route the planner
//! gives each (`route::plan`): the part of the engine every job shares.

use std::path::Path;

use vidcut_core::export::EncoderChoice;
use vidcut_core::model::Hdr;

use super::normalize::{Normalizer, Relayout, down, shown_size};
use crate::decode::VideoReader;
use crate::error::Result;
use crate::ffmpeg::{format, frame};
use crate::route::{Dest, Facts, Ops, Place, Route, Went, plan};

/// What a caller makes of the frames
/// ([`VideoFrames::output`](crate::export::video::VideoFrames::output)).
pub(crate) enum Output {
    /// Fitted into the output on the CPU, the default. Frames a GPU decoded
    /// come down shrunk there first.
    Fitted,
    /// Each frame as decoded: its own size and colors, upright only after
    /// [`rotation`](crate::export::video::VideoFrames::rotation), in a
    /// layout a display takes (see `Relayout`). Black is 8-bit 4:2:0. The
    /// frames of the GPU decoders in `keep` stay in their memory, for a
    /// display that shows them from there. Other GPUs' come down.
    Decoded { keep: Vec<crate::hw::Kind> },
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
    /// For Vulkan's ProRes encoder, opened on the Vulkan device libplacebo
    /// works on, made from NVDEC's CUDA device: pieces decoded on NVDEC are
    /// made into its pictures there ([`Fitting::on_gpu`]), the rest and
    /// black are made on the CPU and go up into the encoder's frames.
    Vulkan,
}

impl Output {
    /// The output for an encoder of `kind` opened on `gpu`, NVENC's CUDA
    /// device and frames, or on a decoder's device whose frames are on
    /// `fit_on`, with `upload` its own frames there. `Fitted` otherwise.
    pub(crate) fn for_encoder(
        kind: EncoderChoice,
        gpu: Option<(crate::cuda::CudaDevice, crate::cuda::CudaFrames)>,
        fit_on: Option<Place>,
        upload: Option<&super::encode::Upload>,
    ) -> Result<Output> {
        Ok(match (gpu, fit_on) {
            (Some((device, frames)), _) => Output::Cuda { device, frames },
            (None, Some(place)) => {
                let into = match upload {
                    // AMF takes Direct3D 11's frames of its own frames
                    // only. SAFETY: the encoder's live frames.
                    Some(up) if kind == EncoderChoice::Amf => {
                        Some(unsafe { crate::hw::Frames::of(up.frames)? })
                    }
                    _ => None,
                };
                Output::FittedOnGpu { place, into }
            }
            (None, None) => Output::Fitted,
        })
    }
}

/// A source's frames on the GPU: the scaler that fits them there, and how
/// its latest picture was made.
pub(crate) struct Fitting {
    /// Fits NVDEC's frames for NVENC or Vulkan's ProRes encoder on the GPU:
    /// built at opening where the planner leaves the first frame there.
    pub(crate) scaler: Option<crate::cuda::CudaPictures>,
    /// Fitting this file's frames on the GPU failed, or it has no scaler
    /// for them: they are made on the CPU.
    fit_failed: bool,
    /// The route that made the latest picture ([`plan`]). The scaler takes
    /// the frame it is given, so that frame can't be planned again.
    pub(crate) route: Option<Route>,
    /// How the latest picture's frame went, for people.
    pub(crate) went: Option<Went>,
}

impl Fitting {
    /// No scaler: NVDEC's frames for NVENC come down to the CPU.
    pub(crate) fn none() -> Self {
        Self {
            scaler: None,
            fit_failed: false,
            route: None,
            went: None,
        }
    }

    /// The fitting of `reader`'s frames into `out` pictures in `format` and
    /// `target`'s dynamic range (`None`: SDR BT.709), for an encoder that
    /// takes frames on `place`: NVENC's CUDA frames, or Vulkan's ProRes
    /// encoder's. `at_once` as for
    /// [`CudaPictures::new`](crate::cuda::CudaPictures::new).
    pub(crate) fn on_gpu(
        reader: &VideoReader,
        out: (u32, u32),
        format: format::Pixel,
        target: Option<Hdr>,
        at_once: bool,
        place: Place,
    ) -> Self {
        // NVDEC's frames fitted for the encoder on the GPU, where the
        // planner leaves the first frame there and the scaler builds
        // (libplacebo needs Vulkan for turned pictures, other matrices than
        // BT.709 and HDR, and makes every picture for Vulkan's encoder).
        let dest = Dest::Encoder { place };
        let planned = reader.cuda().zip(reader.peek()).filter(|(_, first)| {
            let ops = Ops::of(first, reader.rotation(), out, format, target, false);
            plan(&Facts::of(first, &ops, out, dest), dest) == Route::Gpu
        });
        let scaler = planned.and_then(|(d, _)| {
            crate::cuda::CudaPictures::new(d, reader, out, format, target, at_once, place)
                .inspect_err(|e| {
                    crate::debug!(
                        "decoding",
                        "{}: the GPU won't make the encoder's pictures, the CPU makes them: {e}",
                        reader.path().display()
                    )
                })
                .ok()
        });
        let fit_failed = planned.is_some() && scaler.is_none();
        Self {
            scaler,
            fit_failed,
            route: None,
            went: None,
        }
    }

    /// `frame`'s picture, made by the scaler on the GPU. `None`: a scaler
    /// made without `at_once` holds it until the next frame.
    pub(crate) fn scale(&mut self, frame: &frame::Video) -> Result<Option<frame::Video>> {
        let scaler = self.scaler.as_mut().expect("a scaler");
        scaler.push(Some(frame))?;
        scaler.pull()
    }

    /// `route` made the latest picture of `path`'s frames: said under
    /// `VIDCUT_DEBUG` where it changes.
    pub(crate) fn took(&mut self, route: Route, path: &Path) {
        if self.route != Some(route) {
            crate::debug!("decoding", "{}: {route:?}", path.display());
        }
        self.route = Some(route);
    }
}

/// Makes decoded frames into the output's pictures, each by the route the
/// planner gives it.
pub(crate) struct Maker {
    out: (u32, u32, format::Pixel),
    /// The output's dynamic range (`None`: SDR BT.709).
    target: Option<Hdr>,
    /// What the frames are made into ([`output`](Self::output)).
    output: Output,
    /// What makes the pictures of each layer of a dissolve's mix: the
    /// first a frame's only one.
    stages: Vec<Stage>,
    /// The pictures being made go into a dissolve's mix.
    mixing: bool,
}

/// What makes one layer's pictures, kept from frame to frame.
struct Stage {
    /// Fits frames into the output on the CPU, in every output but the
    /// decoded one.
    normalizer: Option<Normalizer>,
    /// Shrinks the frames a GPU decoded on the GPU before they come down to
    /// be fitted into the output.
    shrinker: Option<crate::hw::Shrinker>,
    /// Brings frames as decoded into a layout a display takes.
    relayout: Relayout,
    /// Keeps the GPU's frames there ([`crate::hw::Keeper`]); `unkept`: that
    /// failed, and they come down from then on.
    keeper: Option<crate::hw::Keeper>,
    unkept: bool,
    /// Fits frames into the output on the GPU ([`Output::FittedOnGpu`]).
    fitter: Option<crate::hw::Fitter>,
}

impl Default for Stage {
    fn default() -> Self {
        Self {
            normalizer: None,
            shrinker: None,
            relayout: Relayout::new(),
            keeper: None,
            unkept: false,
            fitter: None,
        }
    }
}

impl Maker {
    /// Makes frames into `output`'s pictures of `out` (width, height and
    /// layout) in `target`'s dynamic range (`None`: SDR BT.709).
    pub(crate) fn new(out: (u32, u32, format::Pixel), target: Option<Hdr>, output: Output) -> Self {
        Self {
            out,
            target,
            output,
            stages: Vec::new(),
            mixing: false,
        }
    }

    /// The pictures made from now on go into a dissolve's mix, or don't.
    pub(crate) fn set_mixing(&mut self, mixing: bool) {
        self.mixing = mixing;
    }

    /// The pictures being made go into a dissolve's mix.
    pub(crate) fn mixing(&self) -> bool {
        self.mixing
    }

    /// What the frames are made into.
    pub(crate) fn output(&self) -> &Output {
        &self.output
    }

    /// The output's width, height and layout.
    pub(crate) fn out(&self) -> (u32, u32, format::Pixel) {
        self.out
    }

    /// The output's dynamic range (`None`: SDR BT.709).
    pub(crate) fn target(&self) -> Option<Hdr> {
        self.target
    }

    /// Where `frame`'s picture goes.
    fn dest(&self, frame: &frame::Video) -> Dest {
        match &self.output {
            Output::Fitted => Dest::Memory,
            Output::Decoded { keep } => Dest::Display {
                keep: crate::hw::Kind::of_frame(frame).is_some_and(|k| keep.contains(&k)),
            },
            Output::FittedOnGpu { place, .. } => Dest::Encoder { place: *place },
            Output::Cuda { .. } => Dest::Encoder { place: Place::Cuda },
            Output::Vulkan => Dest::Encoder {
                place: Place::Vulkan,
            },
        }
    }

    /// What the planner knows of `frame`, shown turned by `rotation`, of a
    /// source whose fitting is `fitting`, made by `stage`.
    fn facts(
        &self,
        stage: &Stage,
        fitting: &Fitting,
        frame: &frame::Video,
        rotation: u16,
    ) -> Facts {
        let (w, h, layout) = self.out;
        let into = matches!(self.output, Output::FittedOnGpu { into: Some(_), .. });
        let ops = Ops::of(frame, rotation, (w, h), layout, self.target, into);
        Facts {
            fit_failed: fitting.fit_failed,
            keep_failed: stage.unkept,
            mixed: self.mixing,
            ..Facts::of(frame, &ops, (w, h), self.dest(frame))
        }
    }

    /// `frame`, of `path` and shown turned by `rotation`, made into the
    /// output's picture by the route the planner gives it. A route on the
    /// GPU that fails is planned again knowing that, for this frame and the
    /// ones after it. `None`: `fitting`'s scaler holds the picture until
    /// the next frame.
    pub(crate) fn make(
        &mut self,
        fitting: &mut Fitting,
        frame: &frame::Video,
        rotation: u16,
        path: &Path,
    ) -> Result<Option<frame::Video>> {
        self.make_layer(0, fitting, frame, rotation, path)
    }

    /// [`make`](Self::make) for layer `layer` of a dissolve's mix, whose
    /// pictures are made apart from the other layers'.
    pub(crate) fn make_layer(
        &mut self,
        layer: usize,
        fitting: &mut Fitting,
        frame: &frame::Video,
        rotation: u16,
        path: &Path,
    ) -> Result<Option<frame::Video>> {
        if self.stages.len() <= layer {
            self.stages.resize_with(layer + 1, Stage::default);
        }
        let mut stage = std::mem::take(&mut self.stages[layer]);
        let made = self.make_with(&mut stage, fitting, frame, rotation, path);
        self.stages[layer] = stage;
        made
    }

    fn make_with(
        &mut self,
        stage: &mut Stage,
        fitting: &mut Fitting,
        frame: &frame::Video,
        rotation: u16,
        path: &Path,
    ) -> Result<Option<frame::Video>> {
        let (out_w, out_h, out_fmt) = self.out;
        let mut facts = self.facts(stage, fitting, frame, rotation);
        let dest = self.dest(frame);
        let mut route;
        // The size the frame came down from the GPU at.
        let mut came_down_at = None;
        let picture = loop {
            route = plan(&facts, dest);
            // Every other route ends on the CPU, with a frame in memory:
            // this one, or one come down from the GPU.
            let came_down = match route {
                Route::Kept => match crate::hw::Keeper::keep(&mut stage.keeper, frame) {
                    Ok(kept) => break kept,
                    // Down to memory, as before they were kept: a copy that
                    // failed or took two seconds would again.
                    Err(e) => {
                        eprintln!("vidcut: GPU pictures come down to be shown: {e}");
                        stage.unkept = true;
                        stage.keeper = None;
                        facts.keep_failed = true;
                        continue;
                    }
                },
                // NVDEC's frames go to the scaler built when the file
                // opened. A file opened bare has none.
                Route::Gpu if facts.place == Place::Cuda => match fitting.scaler {
                    Some(_) => match fitting.scale(frame)? {
                        Some(picture) => break picture,
                        None => return Ok(None),
                    },
                    None => {
                        fitting.fit_failed = true;
                        facts.fit_failed = true;
                        continue;
                    }
                },
                Route::Gpu => {
                    let into = match &self.output {
                        Output::FittedOnGpu { into, .. } => into.as_ref(),
                        _ => None,
                    };
                    // Fitted upright: a quarter turn swaps the sides.
                    let (w, h) = shown_size(frame, rotation, (out_w, out_h));
                    let fitted = if rotation % 180 == 90 { (h, w) } else { (w, h) };
                    let fit = crate::hw::Fitter::fit(
                        &mut stage.fitter,
                        frame,
                        rotation,
                        fitted,
                        (out_w, out_h),
                        out_fmt,
                        into,
                    );
                    match fit {
                        Ok(picture) => break picture,
                        // A GPU that won't fit them (a driver's limits)
                        // leaves the file's frames to the CPU.
                        Err(e) => {
                            crate::debug!(
                                "decoding",
                                "{}: the GPU won't fit the frames for the encoder, the CPU makes them: {e}",
                                path.display()
                            );
                            fitting.fit_failed = true;
                            facts.fit_failed = true;
                            continue;
                        }
                    }
                }
                Route::Shrunk | Route::Whole => Some(match self.output {
                    // Frames as decoded come down at their own size.
                    Output::Decoded { .. } => crate::hw::download(frame)?,
                    _ => down(&mut stage.shrinker, frame, rotation, (out_w, out_h))?,
                }),
                Route::Cpu => None,
            };
            came_down_at = came_down.as_ref().map(|f| (f.width(), f.height()));
            let frame = came_down.as_ref().unwrap_or(frame);
            if let Output::Decoded { .. } = self.output {
                break stage.relayout.run(frame)?;
            }
            let rebuild = stage
                .normalizer
                .as_ref()
                .is_none_or(|n| !n.matches(frame, rotation));
            if rebuild {
                stage.normalizer = Some(Normalizer::new(
                    frame,
                    rotation,
                    out_w,
                    out_h,
                    out_fmt,
                    self.target,
                )?);
            }
            break stage.normalizer.as_mut().expect("built").run(frame)?;
        };
        fitting.went = Some(match came_down_at {
            Some(size) => Went::Down {
                size,
                whole: size == (frame.width(), frame.height()),
            },
            None if route == Route::Cpu => Went::Cpu,
            None => Went::Stayed,
        });
        fitting.took(route, path);
        Ok(Some(picture))
    }

    /// The pictures `fitting`'s scaler still holds once its last frame went
    /// in. Empty without a scaler.
    pub(crate) fn rest(&mut self, fitting: &mut Fitting) -> Result<Vec<frame::Video>> {
        let mut rest = Vec::new();
        if let Some(scaler) = fitting.scaler.as_mut() {
            scaler.push(None)?;
            while let Some(picture) = scaler.pull()? {
                rest.push(picture);
            }
        }
        Ok(rest)
    }
}
