use std::path::Path;

use vidcut_core::export::EncoderChoice;
use vidcut_core::model::{Hdr, Sharpness};

use super::normalize::{Normalizer, Relayout, down, shown_size};
use crate::decode::Reading;
use crate::decode::VideoReader;
use crate::error::Result;
use crate::ffmpeg::{format, frame};
use crate::route::{Dest, Facts, Ops, Place, Route, Went, plan};

pub(crate) enum Output {
    Fitted,
    Decoded {
        keep: Vec<crate::hw::Kind>,
    },
    FittedOnGpu {
        place: Place,
        into: Option<crate::hw::Frames>,
    },
    Cuda {
        device: crate::cuda::CudaDevice,
        frames: crate::cuda::CudaFrames,
    },
    Vulkan,
}

impl Output {
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
                    // SAFETY: `up.frames` is the encoder's live frames reference.
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

pub(crate) struct Fitting {
    pub(crate) scaler: Option<crate::cuda::CudaPictures>,
    fit_failed: bool,
    pub(crate) route: Option<Route>,
    pub(crate) went: Option<Went>,
}

impl Fitting {
    pub(crate) fn none() -> Self {
        Self {
            scaler: None,
            fit_failed: false,
            route: None,
            went: None,
        }
    }

    pub(crate) fn on_gpu(
        reader: &VideoReader,
        out: (u32, u32),
        format: format::Pixel,
        target: Option<Hdr>,
        at_once: bool,
        place: Place,
        sharp: Option<Sharpness>,
    ) -> Self {
        let dest = Dest::Encoder { place };
        let planned = reader
            .decoders_cuda()
            .zip(reader.peek())
            .filter(|_| reader.frames_stay())
            .filter(|(_, first)| {
                let ops = Ops::of(first, reader.rotation(), out, format, target, false);
                plan(&Facts::of(first, &ops, out, dest), dest) == Route::Gpu
            });
        let scaler = planned.and_then(|(d, _)| {
            crate::cuda::CudaPictures::new(d, reader, out, format, target, (at_once, place), sharp)
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

    pub(crate) fn scale(&mut self, frame: &frame::Video) -> Result<Option<frame::Video>> {
        let scaler = self.scaler.as_mut().expect("a scaler");
        scaler.push(Some(&super::video::frame_ref(frame)?))?;
        scaler.pull()
    }

    pub(crate) fn took(&mut self, route: Route, path: &Path) {
        if self.route != Some(route) {
            crate::debug!("decoding", "{}: {route:?}", path.display());
        }
        self.route = Some(route);
    }
}

pub(crate) struct Maker {
    out: (u32, u32, format::Pixel),
    target: Option<Hdr>,
    output: Output,
    stages: Vec<Stage>,
    mixing: bool,
    shrink: Option<(u32, u32)>,
    reading: Reading,
}

struct Stage {
    normalizer: Option<Normalizer>,
    size: Option<(u32, u32)>,
    made_at: Option<(u32, u32)>,
    shrinker: Option<crate::hw::Shrinker>,
    relayout: Relayout,
    keeper: Option<crate::hw::Keeper>,
    unkept: bool,
    fitter: Option<crate::hw::Fitter>,
    reading: Option<Reading>,
    sharp: Option<Sharpness>,
}

impl Default for Stage {
    fn default() -> Self {
        Self {
            normalizer: None,
            size: None,
            made_at: None,
            shrinker: None,
            relayout: Relayout::new(),
            keeper: None,
            unkept: false,
            fitter: None,
            reading: None,
            sharp: None,
        }
    }
}

impl Maker {
    pub(crate) fn new(
        out: (u32, u32, format::Pixel),
        target: Option<Hdr>,
        output: Output,
        reading: Reading,
    ) -> Self {
        Self {
            out,
            target,
            output,
            stages: Vec::new(),
            mixing: false,
            shrink: None,
            reading,
        }
    }

    pub(crate) fn shrink_to(&mut self, size: Option<(u32, u32)>) {
        self.shrink = size;
    }

    pub(crate) fn shrinks(&self) -> Option<(u32, u32)> {
        self.shrink
    }

    pub(crate) fn clear_stages(&mut self) {
        self.stages.clear();
    }

    pub(crate) fn size_layer(&mut self, layer: usize, size: Option<(u32, u32)>) {
        if self.stages.len() <= layer {
            self.stages.resize_with(layer + 1, Stage::default);
        }
        self.stages[layer].size = size;
    }

    pub(crate) fn sharpen_layer(&mut self, layer: usize, sharp: Option<Sharpness>) {
        if self.stages.len() <= layer {
            self.stages.resize_with(layer + 1, Stage::default);
        }
        let stage = &mut self.stages[layer];
        if stage.sharp != sharp {
            stage.sharp = sharp;
            stage.normalizer = None;
        }
    }

    pub(crate) fn set_mixing(&mut self, mixing: bool) {
        self.mixing = mixing;
    }

    pub(crate) fn mixing(&self) -> bool {
        self.mixing
    }

    pub(crate) fn output(&self) -> &Output {
        &self.output
    }

    pub(crate) fn out(&self) -> (u32, u32, format::Pixel) {
        self.out
    }

    pub(crate) fn reading(&self) -> Reading {
        self.reading
    }

    pub(crate) fn read_as(&mut self, reading: Reading) {
        self.reading = reading;
        self.stages.clear();
    }

    pub(crate) fn target(&self) -> Option<Hdr> {
        self.target
    }

    fn dest(&self, frame: &frame::Video) -> Dest {
        match &self.output {
            Output::Fitted => Dest::Memory,
            Output::Decoded { keep } => Dest::Display {
                keep: self.shrink.is_none()
                    && crate::hw::Kind::of_frame(frame).is_some_and(|k| keep.contains(&k)),
            },
            Output::FittedOnGpu { place, .. } => Dest::Encoder { place: *place },
            Output::Cuda { .. } => Dest::Encoder { place: Place::Cuda },
            Output::Vulkan => Dest::Encoder {
                place: Place::Vulkan,
            },
        }
    }

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

    pub(crate) fn make(
        &mut self,
        fitting: &mut Fitting,
        frame: &frame::Video,
        rotation: u16,
        path: &Path,
    ) -> Result<Option<frame::Video>> {
        self.make_layer(0, fitting, frame, rotation, path, self.reading)
    }

    pub(crate) fn make_layer(
        &mut self,
        layer: usize,
        fitting: &mut Fitting,
        frame: &frame::Video,
        rotation: u16,
        path: &Path,
        reading: Reading,
    ) -> Result<Option<frame::Video>> {
        if self.stages.len() <= layer {
            self.stages.resize_with(layer + 1, Stage::default);
        }
        let mut stage = std::mem::take(&mut self.stages[layer]);
        let made = self.make_with(&mut stage, fitting, frame, rotation, path, reading);
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
        reading: Reading,
    ) -> Result<Option<frame::Video>> {
        if stage.reading.replace(reading).is_some_and(|r| r != reading) {
            stage.normalizer = None;
            stage.relayout = Relayout::new();
        }
        let (out_w, out_h) = stage.size.unwrap_or((self.out.0, self.out.1));
        let out_fmt = self.out.2;
        let mut facts = self.facts(stage, fitting, frame, rotation);
        if stage.size.is_some() {
            facts.mixes = false;
        }
        let dest = self.dest(frame);
        let mut route;
        let mut came_down_at = None;
        let picture = loop {
            route = plan(&facts, dest);
            let came_down = match route {
                Route::Kept => match crate::hw::Keeper::keep(&mut stage.keeper, frame) {
                    Ok(kept) => break kept,
                    Err(e) => {
                        eprintln!("vidcut: GPU pictures come down to be shown: {e}");
                        stage.unkept = true;
                        stage.keeper = None;
                        facts.keep_failed = true;
                        continue;
                    }
                },
                Route::Gpu if matches!(facts.place, Place::Cuda | Place::Vulkan) => {
                    match fitting.scaler {
                        Some(_) => match fitting.scale(frame)? {
                            Some(picture) => break picture,
                            None => return Ok(None),
                        },
                        None => {
                            fitting.fit_failed = true;
                            facts.fit_failed = true;
                            continue;
                        }
                    }
                }
                Route::Gpu => {
                    let into = match &self.output {
                        Output::FittedOnGpu { into, .. } => into.as_ref(),
                        _ => None,
                    };
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
                    Output::Decoded { .. } => match self.shrink {
                        Some(size) => down(&mut stage.shrinker, frame, rotation, size)?,
                        None => crate::hw::download(frame)?,
                    },
                    _ => down(&mut stage.shrinker, frame, rotation, (out_w, out_h))?,
                }),
                Route::Cpu => None,
            };
            came_down_at = came_down.as_ref().map(|f| (f.width(), f.height()));
            let frame = came_down.as_ref().unwrap_or(frame);
            if let Output::Decoded { .. } = self.output {
                break crate::buffers::owned(&stage.relayout.run(frame, reading)?)?;
            }
            let rebuild = stage.made_at != Some((out_w, out_h))
                || stage
                    .normalizer
                    .as_ref()
                    .is_none_or(|n| !n.matches(frame, rotation));
            if rebuild {
                stage.made_at = Some((out_w, out_h));
                stage.normalizer = Some(Normalizer::upscaling(
                    frame,
                    rotation,
                    (out_w, out_h),
                    out_fmt,
                    self.target,
                    reading,
                    stage.sharp,
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
