//! The video of a render plan: frames picked by the sampling rule and
//! normalized to the output size and pixel format.
//!
//! Sampling rule (docs/decisions/export.md): output frame `k` of a source piece
//! shows the first source frame whose timestamp is at least
//! `origin + (source_frame + k) / fps − ½ frame`. That is frame-exact for
//! sources at the project rate and nearest-frame sampling otherwise.

use std::path::Path;

use vidcut_core::model::Hdr;
use vidcut_core::render::{Image, VideoPiece};
use vidcut_core::time::Rational;

use super::ahead::Ahead;
use super::normalize::{Normalizer, Relayout, displayable, down, fit};
use crate::decode::{Input, VideoReader};
use crate::error::{Error, Result};
use crate::ffmpeg::{self, format, frame};

/// The sampling rule's time (seconds) for source position `pos` (project
/// frames from `origin`) at `fps`: the first source frame from then on is
/// shown.
pub(crate) fn target(fps: Rational, origin: f64, pos: i64) -> f64 {
    origin + fps.frames_to_secs(pos) - 0.5 * fps.frames_to_secs(1)
}

/// How a piece's pictures are read: its file, or its image sequence.
pub(crate) fn input_of(path: &Path, image: Option<&Image>) -> Input {
    match image {
        Some(Image::Sequence {
            pattern,
            first,
            rate,
            ..
        }) => Input::Sequence {
            pattern: pattern.clone(),
            first: *first,
            rate: (rate.num() as i32, rate.den() as i32),
        },
        Some(Image::Still) | None => Input::File(path.to_path_buf()),
    }
}

/// What a piece's file holds: video, a still's one picture, or an image
/// sequence of `count` pictures.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pictures {
    Video,
    Still,
    Sequence { count: u64 },
}

impl Pictures {
    fn of(image: Option<&Image>) -> Self {
        match image {
            Some(Image::Still) => Self::Still,
            Some(Image::Sequence { count, .. }) => Self::Sequence { count: *count },
            None => Self::Video,
        }
    }
}

/// A new reference to `src`'s buffers (no pixel copy).
pub(crate) fn frame_ref(src: &frame::Video) -> Result<frame::Video> {
    let mut dst = frame::Video::empty();
    let ret = unsafe { ffmpeg::ffi::av_frame_ref(dst.as_mut_ptr(), src.as_ptr()) };
    if ret < 0 {
        return Err(Error::Ffmpeg {
            context: "referencing a frame".into(),
            source: ffmpeg::Error::from(ret),
        });
    }
    Ok(dst)
}

/// An open source file, read for one piece or several in a row.
struct Source {
    reader: VideoReader,
    /// Seconds per tick of the reader's time base.
    tick: f64,
    /// The current piece's origin and first source frame.
    origin: f64,
    source_frame: i64,
    /// The latest sampling target (seconds); every frame before `current` is
    /// earlier than it.
    target: f64,
    /// Current source frame (decoded) and its timestamp in seconds.
    current: Option<(frame::Video, f64)>,
    /// `current`'s dynamic HDR metadata, kept apart: the GPU's scaler takes
    /// the frame itself, side data and all.
    dynamic: frame::Video,
    /// Measures `current` when it has no HDR10+, as it is decoded (before
    /// the GPU's scaler takes it): for a file not measured ahead.
    meter: Option<crate::light::Meter>,
    /// `current`'s light, measured by `meter`.
    light: Option<crate::light::FrameLight>,
    /// `current`, normalized.
    normalized: Option<frame::Video>,
    /// `normalized` was fitted on the GPU ([`Output::FittedOnGpu`]).
    fitted: bool,
    /// Fitting this file's frames on the GPU failed: they are made on the
    /// CPU.
    fit_failed: bool,
    /// A still keeps its one picture and is never sought; a sequence knows
    /// how many pictures it has.
    pictures: Pictures,
    /// Decoded on NVDEC, and scaled on the GPU by `scaler` (built for the
    /// first frame), when the export's frames stay on the GPU.
    on_gpu: bool,
    scaler: Option<crate::cuda::CudaPictures>,
    /// The GPU the reader decodes on: NVDEC for an export on the GPU
    /// (`on_gpu`), or any for frames as decoded (`decode_on`).
    decoder: Option<crate::hw::Kind>,
}

/// What a source needs to decode on NVDEC for an export whose frames stay
/// on the GPU: the device, and the output's size, format and dynamic range.
#[derive(Clone)]
pub(crate) struct GpuSide {
    device: crate::cuda::CudaDevice,
    out: (u32, u32),
    format: format::Pixel,
    target: Option<Hdr>,
}

/// How far past the current frame (seconds) a later piece of the same file
/// may start for decoding on to it to beat a seek.
const DECODE_AHEAD: f64 = 1.0;

/// Memory for the pictures of a piece walking its source backward made at
/// once ([`VideoFrames::reversed_frame`]), at most `REVERSED_MOST` of them;
/// `REVERSED_ON_GPU` for an encoder's frames on the GPU, whose pools are
/// small.
const REVERSED_BYTES: usize = 256 << 20;
const REVERSED_MOST: i64 = 64;
const REVERSED_ON_GPU: i64 = 4;

/// A picture of a piece walking its source backward, made ahead.
struct Reversed {
    input: Input,
    pos: i64,
    frame: frame::Video,
    /// Its source frame's Dolby Vision, for black after it.
    dolby_vision: frame::Video,
}

impl Source {
    /// Opens `path` with `current` at the first frame for `target`. The
    /// decoding up to there is most of the cost of starting a piece, so the
    /// next file opens on another thread while this one encodes.
    ///
    /// A still's picture is already decoded by [`VideoReader::open_input`].
    /// Seeking would reopen the file and decode it again; a still is never
    /// sought.
    ///
    /// `measure`: frames without HDR10+ are measured as they are decoded.
    #[allow(clippy::too_many_arguments)]
    fn open(
        input: &Input,
        origin: f64,
        source_frame: i64,
        target: f64,
        pictures: Pictures,
        gpu: Option<&GpuSide>,
        decoder: Option<&crate::hw::GpuDecoder>,
        measure: bool,
    ) -> Result<Source> {
        let video = pictures == Pictures::Video;
        // NVENC's pieces decode on NVDEC on a stream of their own: the next
        // piece opens, and makes its first picture, on another thread while
        // this one encodes, and frames go to the encoder once their
        // stream's work is done (`share::wait_for`).
        let nvdec = gpu.filter(|_| video);
        let nvdec = nvdec.and_then(|g| g.device.with_own_stream().ok());
        let nvdec = nvdec.map(crate::hw::GpuDecoder::nvdec);
        let reader = match nvdec.as_ref().or(decoder).filter(|_| video) {
            Some(d) => VideoReader::open_gpu_or_cpu(input, d)?,
            None => VideoReader::open_input(input)?,
        };
        // NVDEC's frames scaled for NVENC on the GPU, where the scaler
        // builds (libplacebo needs Vulkan for turned pictures, other
        // matrices than BT.709, and HDR).
        let scaler = gpu.zip(reader.cuda()).and_then(|(g, d)| {
            crate::cuda::CudaPictures::new(d, &reader, g.out, g.format, g.target, true).ok()
        });
        let on_gpu = scaler.is_some();
        let decoder = reader.gpu();
        let tb = reader.time_base();
        let mut source = Source {
            reader,
            tick: f64::from(tb.numerator()) / f64::from(tb.denominator()),
            origin,
            source_frame,
            target,
            current: None,
            dynamic: frame::Video::empty(),
            meter: measure.then(crate::light::Meter::new),
            light: None,
            normalized: None,
            fitted: false,
            fit_failed: false,
            pictures,
            on_gpu,
            scaler,
            decoder,
        };
        if pictures == Pictures::Still {
            source.advance(target)?;
        } else {
            source.seek(target)?;
            source.advance(target)?;
        }
        // The GPU's first picture too: libplacebo compiles its shaders for
        // it, which is better done here, ahead of the piece.
        if source.on_gpu {
            source.scale_on_gpu()?;
        }
        Ok(source)
    }

    /// `normalized` from the current frame, on the GPU.
    fn scale_on_gpu(&mut self) -> Result<()> {
        let (frame, _) = self.current.as_ref().expect("have a frame");
        let scaler = self.scaler.as_mut().expect("built at opening");
        scaler.push(Some(frame))?;
        let picture = scaler
            .pull()?
            .ok_or_else(|| Error::Invalid("the GPU scaler gave no picture".into()))?;
        self.normalized = Some(picture);
        Ok(())
    }

    /// Opens `input` without decoding anything yet: on the GPU (`decoder`)
    /// when it decodes the stream. `measure` as for [`open`](Self::open).
    fn bare(
        input: &Input,
        origin: f64,
        pictures: Pictures,
        decoder: Option<&crate::hw::GpuDecoder>,
        measure: bool,
    ) -> Result<Source> {
        let reader = match decoder {
            Some(d) => VideoReader::open_gpu_or_cpu(input, d)?,
            None => VideoReader::open_input(input)?,
        };
        let decoder = reader.gpu();
        let tb = reader.time_base();
        Ok(Source {
            reader,
            tick: f64::from(tb.numerator()) / f64::from(tb.denominator()),
            origin,
            source_frame: 0,
            target: 0.0,
            current: None,
            dynamic: frame::Video::empty(),
            meter: measure.then(crate::light::Meter::new),
            light: None,
            normalized: None,
            fitted: false,
            fit_failed: false,
            pictures,
            on_gpu: false,
            scaler: None,
            decoder,
        })
    }

    fn seek(&mut self, target: f64) -> Result<()> {
        self.reader.seek((target / self.tick).floor() as i64)?;
        self.target = target;
        self.current = None;
        self.light = None;
        self.normalized = None;
        Ok(())
    }

    /// Moves `current` to the first frame at or after `target` (1 µs of slack
    /// for float noise). Past the end of a file it stays on the last frame,
    /// which is how a still keeps its one picture, and how a sequence's last
    /// picture lasts its frame (a project rate above the sequence's asks for
    /// a time past it). A sequence that ends before its last picture fails:
    /// a file is missing, and repeating the picture before it would hide the
    /// hole. The frame it moves to is measured by `meter` when it has no
    /// HDR10+. Returns whether it moved.
    fn advance(&mut self, target: f64) -> Result<bool> {
        // Back (a piece walking its source backward, decision D48): from
        // the keyframe before. Just sought, the frames from where it
        // landed come next.
        let behind = match &self.current {
            Some(_) => target < self.target - 1e-6,
            None => self
                .reader
                .passes_before()
                .is_some_and(|p| p as f64 * self.tick > target + 1e-6),
        };
        if self.pictures != Pictures::Still && behind {
            self.seek(target)?;
        }
        self.target = target;
        let mut changed = false;
        while self
            .current
            .as_ref()
            .is_none_or(|(_, t)| *t < target - 1e-6)
        {
            match self.reader.next_frame()? {
                Some(f) => {
                    let t = f.pts().unwrap_or(0) as f64 * self.tick;
                    self.dynamic = frame::Video::empty();
                    let all = crate::hdr::Dynamic {
                        hdr10plus: true,
                        dolby_vision: true,
                    };
                    crate::hdr::copy_dynamic(&f, &mut self.dynamic, all)?;
                    self.current = Some((f, t));
                    self.light = None;
                    changed = true;
                }
                None => {
                    // Picture `k` of a sequence has timestamp `k`.
                    let shown = self.current.as_ref().map(|(f, _)| f.pts().unwrap_or(0));
                    let missing = match (shown, self.pictures) {
                        (None, _) => String::new(),
                        (Some(k), Pictures::Sequence { count }) if k + 1 < count as i64 => {
                            let first = match self.reader.input() {
                                Input::Sequence { first, .. } => *first,
                                Input::File(_) => 0,
                            };
                            format!(
                                ": picture {} of {count} (file number {}) is missing",
                                k + 1,
                                first + k as u64 + 1
                            )
                        }
                        // The last picture: of a video or a still, or of a
                        // whole sequence.
                        (Some(_), _) => break,
                    };
                    return Err(Error::Invalid(format!(
                        "{} ends before {target:.3} s{missing}",
                        self.reader.path().display()
                    )));
                }
            }
        }
        if changed
            && let Some(meter) = self.meter.as_mut()
            && !crate::hdr::has_hdr10plus(&self.dynamic)
        {
            let (f, _) = self.current.as_ref().expect("have a frame");
            // A frame that can't be measured gets none.
            self.light = meter.measure(f).ok();
        }
        Ok(changed)
    }

    /// Readies the file for another piece starting at `target`: a piece that
    /// starts a little ahead decodes on to it, any other seeks. A still is
    /// never sought; its one picture already stands in for every frame.
    fn reposition(&mut self, origin: f64, source_frame: i64, target: f64) -> Result<()> {
        self.origin = origin;
        self.source_frame = source_frame;
        if self.pictures == Pictures::Still {
            return Ok(());
        }
        let ahead = match &self.current {
            Some((_, t)) => target >= self.target && target - t <= DECODE_AHEAD,
            // Just sought (a keyframe looked for, `VideoFrames::keyframe_time`):
            // the frame it landed on comes next, and decoding on from it
            // reaches any target not before it. Up to the target sought,
            // no keyframe lies between: a seek would land on the same one.
            None => self
                .reader
                .pending_pts()
                .map(|p| p as f64 * self.tick)
                .is_some_and(|t| {
                    t <= target + 1e-6
                        && (target <= self.target + 1e-6 || target - t <= DECODE_AHEAD)
                }),
        };
        if !ahead {
            self.seek(target)?;
        }
        Ok(())
    }
}

/// Produces the plan's video frames in order. A piece from the same file as
/// the one before it reads on with the same decoder; a piece from another
/// file is opened while the piece before it encodes.
pub struct VideoFrames {
    pieces: Vec<VideoPiece>,
    fps: Rational,
    out: (u32, u32, format::Pixel),
    piece: usize,
    /// Frame index within the current piece.
    k: i64,
    source: Option<Source>,
    /// The next file, opened ahead; named by where it opens.
    ahead: Ahead<Source, (Input, u64, i64)>,
    black: Option<frame::Video>,
    /// The output's dynamic range (`None`: SDR BT.709).
    target: Option<Hdr>,
    /// The GPU frames decode on ([`decode_on`](Self::decode_on)).
    decoder: Option<crate::hw::GpuDecoder>,
    /// What the frames are made into ([`output`](Self::output)).
    output: Output,
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
    /// Frames given so far that the GPU made, for the encoder there.
    gpu_made: i64,
    /// The clockwise turn that shows the last frame given upright.
    rotation: u16,
    /// The GPU that decoded the last frame given, and what its file holds
    /// and what decoded it (`VideoReader::decoding`).
    last_decoder: Option<crate::hw::Kind>,
    last_decoding: Option<std::sync::Arc<crate::decode::Decoding>>,
    /// Frames given so far whose source frame a GPU decoded.
    gpu_decoded: i64,
    /// The dynamic HDR metadata each output frame keeps from its source
    /// frame (decision D46). Other HDR metadata of the sources goes.
    dynamic: crate::hdr::Dynamic,
    /// Measured light of the files whose frames without HDR10+ get it
    /// made, by path (`with_light`).
    light:
        std::collections::HashMap<std::path::PathBuf, std::sync::Arc<crate::light::LightAnalysis>>,
    /// The Dolby Vision of the last source frame given: black after it
    /// takes it.
    last_dolby_vision: frame::Video,
    /// Pictures of a piece walking its source backward, made ahead.
    reversed: Vec<Reversed>,
}

/// What a caller makes of the frames ([`VideoFrames::output`]).
pub(crate) enum Output {
    /// Fitted into the output on the CPU, the default. Frames a GPU decoded
    /// come down shrunk there first.
    Fitted,
    /// Each frame as decoded: its own size and colors, upright only after
    /// [`rotation`](VideoFrames::rotation), in a layout a display takes (see
    /// `Relayout`). Black is 8-bit 4:2:0. With `keep_on_gpu`, the GPU's
    /// frames stay in its memory in NV12 or P010, for a display that shows
    /// them from there.
    Decoded { keep_on_gpu: bool },
    /// Fitted for an encoder on the GPU's device: frames in the output's
    /// colors that the GPU fits ([`crate::hw::Fitter::fits`]) are fitted
    /// there, Direct3D 11's into `into`, the encoder's frames. The rest are
    /// fitted on the CPU.
    FittedOnGpu { into: Option<crate::hw::Frames> },
    /// For NVENC, in CUDA memory from `frames`, made for the output's size
    /// and format (NV12 or P010): pieces decoded on NVDEC are scaled there,
    /// the rest and black are made on the CPU and go up.
    Cuda {
        device: crate::cuda::CudaDevice,
        frames: crate::cuda::CudaFrames,
    },
}

impl VideoFrames {
    /// `target`: the output's dynamic range (`None`: SDR BT.709).
    pub fn new(
        pieces: &[VideoPiece],
        fps: Rational,
        width: u32,
        height: u32,
        pixel: format::Pixel,
        target: Option<Hdr>,
    ) -> Self {
        Self {
            pieces: pieces.to_vec(),
            fps,
            out: (width, height, pixel),
            piece: 0,
            k: 0,
            source: None,
            ahead: Ahead::new(),
            black: None,
            target,
            decoder: None,
            output: Output::Fitted,
            normalizer: None,
            shrinker: None,
            relayout: Relayout::new(),
            keeper: None,
            unkept: false,
            fitter: None,
            gpu_made: 0,
            rotation: 0,
            last_decoder: None,
            last_decoding: None,
            gpu_decoded: 0,
            dynamic: Default::default(),
            light: Default::default(),
            last_dolby_vision: frame::Video::empty(),
            reversed: Vec::new(),
        }
    }

    /// Makes the frames into `output`. Frames are picked by the sampling
    /// rule whatever the output.
    pub(crate) fn output(mut self, output: Output) -> Self {
        if matches!(output, Output::Decoded { .. }) {
            self.out.2 = format::Pixel::YUV420P;
        }
        self.output = output;
        self
    }

    /// Frames decode on `gpu` where it decodes the stream
    /// ([`VideoReader::open_gpu_or_cpu`]): the same frames, with the same
    /// timestamps and colors. Frames as decoded ([`Output::Decoded`]) are
    /// copied down to memory at their own size; frames fitted into the
    /// output come down shrunk on the GPU to the size they are shown at,
    /// where it is smaller ([`down`]). Other streams, stills and image
    /// sequences decode on the CPU, and pieces NVDEC takes for NVENC
    /// ([`Output::Cuda`]) stay with it.
    pub fn decode_on(mut self, gpu: crate::hw::GpuDecoder) -> Self {
        self.decoder = Some(gpu);
        self
    }

    /// The GPU that decoded the last frame given (`None` for black, and for
    /// frames decoded on the CPU).
    pub fn decoded_on(&self) -> Option<crate::hw::Kind> {
        self.last_decoder
    }

    /// What the file of the last frame given holds and what decoded it
    /// (`VideoReader::decoding`); `None` for black.
    pub fn decoding(&self) -> Option<std::sync::Arc<crate::decode::Decoding>> {
        self.last_decoding.clone()
    }

    /// The clockwise turn (0, 90, 180 or 270) that shows the last frame
    /// given upright: its file's display matrix; 0 for black.
    pub fn rotation(&self) -> u16 {
        self.rotation
    }

    /// Output frames keep `dynamic` HDR metadata from the frames they are
    /// made from.
    pub fn with_dynamic(mut self, dynamic: crate::hdr::Dynamic) -> Self {
        self.dynamic = dynamic;
        self
    }

    /// Output frames keeping HDR10+ get it made from `light`, the measured
    /// light of their source files by path, where their source frame has
    /// none; black gets HDR10+ of no light. Frames of other files without
    /// it are measured as they are decoded.
    pub fn with_light(
        mut self,
        light: std::collections::HashMap<
            std::path::PathBuf,
            std::sync::Arc<crate::light::LightAnalysis>,
        >,
    ) -> Self {
        self.light = light;
        self
    }

    /// Frames decoded and scaled on the GPU so far, on their way to the
    /// encoder there.
    pub fn gpu_frames(&self) -> i64 {
        self.gpu_made
    }

    /// What a source decoding for NVENC needs ([`Output::Cuda`]).
    fn gpu_side(&self) -> Option<GpuSide> {
        let Output::Cuda { device, .. } = &self.output else {
            return None;
        };
        let (w, h, format) = self.out;
        Some(GpuSide {
            device: device.clone(),
            out: (w, h),
            format,
            target: self.target,
        })
    }

    /// Frames given so far whose source frame a GPU decoded, whether it
    /// stayed there or came down.
    pub fn gpu_decoded(&self) -> i64 {
        self.gpu_decoded
    }

    /// The rate frames are sampled at.
    pub fn fps(&self) -> Rational {
        self.fps
    }

    /// The next output frame (pts unset), or `None` after the last one.
    pub fn next_frame(&mut self) -> Result<Option<frame::Video>> {
        Ok(self
            .next(true)?
            .map(|f| f.expect("a picture was asked for")))
    }

    /// Moves past the next output frame without making its picture: it is
    /// decoded, since decoding on needs it, but not scaled. Returns whether
    /// there was a frame.
    pub fn skip_frame(&mut self) -> Result<bool> {
        Ok(self.next(false)?.is_some())
    }

    /// The next output frame, with its picture when `picture` is set.
    fn next(&mut self, picture: bool) -> Result<Option<Option<frame::Video>>> {
        loop {
            let Some(piece) = self.pieces.get(self.piece) else {
                return Ok(None);
            };
            if self.k >= piece.frames() {
                // The file stays open: a later piece may read it too.
                self.piece += 1;
                self.k = 0;
                continue;
            }
            let k = self.k;
            self.k += 1;
            return match piece.clone() {
                VideoPiece::Black { .. } if !picture => Ok(Some(None)),
                VideoPiece::Black { .. } => {
                    self.rotation = 0;
                    self.last_decoder = None;
                    self.last_decoding = None;
                    let black = frame_ref(self.black()?)?;
                    let mut out = match &self.output {
                        Output::Cuda { frames, .. } => frames.upload(&black)?,
                        _ => black,
                    };
                    if self.dynamic.hdr10plus {
                        crate::light::FrameLight::default()
                            .hdr10plus()
                            .attach(&mut out)?;
                    }
                    if self.dynamic.dolby_vision {
                        let dv = crate::hdr::Dynamic {
                            hdr10plus: false,
                            dolby_vision: true,
                        };
                        crate::hdr::copy_dynamic(&self.last_dolby_vision, &mut out, dv)?;
                    }
                    Ok(Some(Some(out)))
                }
                VideoPiece::Source {
                    path,
                    origin,
                    source_frame,
                    image,
                    pace,
                    ..
                } => {
                    // A still's one picture serves every frame: it is never
                    // sought (an image file has nothing past it).
                    let pictures = Pictures::of(image.as_ref());
                    let still = pictures == Pictures::Still;
                    let backward = pace.speed < 0.0 && !still;
                    if k == 0 {
                        let from = if still { 0 } else { source_frame };
                        self.start_piece(&input_of(&path, image.as_ref()), origin, from, pictures)?;
                        if !backward {
                            self.reversed.clear();
                        }
                    }
                    let k = if still { 0 } else { k };
                    if picture {
                        self.rotation = self.source.as_ref().map_or(0, |s| s.reader.rotation());
                        self.last_decoder = self.source.as_ref().and_then(|s| s.decoder);
                        self.last_decoding =
                            self.source.as_ref().map(|s| s.reader.decoding().clone());
                        self.gpu_decoded += i64::from(self.last_decoder.is_some());
                        if backward {
                            return self.reversed_frame(k).map(|f| Some(Some(f)));
                        }
                        self.source_frame(k).map(|f| Some(Some(f)))
                    } else if backward {
                        // Made when one is asked for, from a keyframe.
                        Ok(Some(None))
                    } else {
                        self.pass_source_frame(k).map(|()| Some(None))
                    }
                }
            };
        }
    }

    /// Forgets where the open file stands, so the next piece of it seeks
    /// rather than decoding on: after decoding was interrupted (a cancel
    /// flag), the reader may stand anywhere. A still keeps its picture.
    pub fn forget_position(&mut self) {
        if let Some(s) = self.source.as_mut()
            && s.pictures != Pictures::Still
        {
            s.current = None;
            s.normalized = None;
        }
    }

    /// The time (seconds) of the keyframe decoding starts from for the
    /// first frame of `piece`, a video file's: a seek and one decoded frame.
    /// The reader stays there: frames read from the keyframe on decode on
    /// without seeking again. `None` for black, stills and image sequences.
    pub fn keyframe_time(&mut self, piece: &VideoPiece) -> Result<Option<f64>> {
        let VideoPiece::Source {
            path,
            origin,
            source_frame,
            image: None,
            ..
        } = piece
        else {
            return Ok(None);
        };
        let input = input_of(path, None);
        let target = self.target(*origin, *source_frame);
        let mut source = match self.source.take() {
            Some(s) if s.reader.input() == &input => s,
            _ => Source::bare(
                &input,
                *origin,
                Pictures::Video,
                self.decoder.as_ref(),
                self.measures(&input),
            )?,
        };
        // Interrupted, the file stays open, its position unknown.
        let sought = source.seek(target);
        // The window that starts there decodes on from it.
        source.reader.from_keyframe();
        let at = source.reader.pending_pts().map(|p| p as f64 * source.tick);
        self.source = Some(source);
        sought?;
        Ok(at)
    }

    /// Makes `piece` alone the plan, its frames next: the open file stays
    /// open, so a piece of it starting shortly after the frame before
    /// decodes on to it instead of seeking.
    pub fn read_piece(&mut self, piece: &VideoPiece) {
        self.read_pieces(std::slice::from_ref(piece));
    }

    /// Makes `pieces` the plan, their frames next, as
    /// [`read_piece`](Self::read_piece) does for one: the open file stays
    /// open, and the next file opens ahead as it would for a new plan.
    pub fn read_pieces(&mut self, pieces: &[VideoPiece]) {
        self.pieces = pieces.to_vec();
        self.piece = 0;
        self.k = 0;
        self.ahead.forget();
    }

    fn black(&mut self) -> Result<&frame::Video> {
        if self.black.is_none() {
            let (w, h, pix) = self.out;
            let mut f = frame::Video::new(pix, w, h);
            fill_black(&mut f)?;
            self.black = Some(f);
        }
        Ok(self.black.as_ref().expect("just set"))
    }

    /// Seconds of the sampling-rule target for source position `pos`.
    fn target(&self, origin: f64, pos: i64) -> f64 {
        target(self.fps, origin, pos)
    }

    /// Whether frames of `input` without HDR10+ are measured as they are
    /// decoded, for HDR10+ made from their light: HDR10+ is kept and the
    /// file wasn't measured ahead (`with_light`), its first frame having
    /// HDR10+.
    fn measures(&self, input: &Input) -> bool {
        self.dynamic.hdr10plus && !self.light.contains_key(input.path())
    }

    /// Readies the current piece's file: the open one if it is the same
    /// file, the one opened ahead for this piece, or a new one. Then starts
    /// opening the next file.
    fn start_piece(
        &mut self,
        input: &Input,
        origin: f64,
        source_frame: i64,
        pictures: Pictures,
    ) -> Result<()> {
        let target = self.target(origin, source_frame);
        let source = match self.source.take() {
            Some(mut s) if s.reader.input() == input => {
                if let Err(e) = s.reposition(origin, source_frame, target) {
                    // Interrupted, say: the file stays open, its position
                    // unknown.
                    s.current = None;
                    s.normalized = None;
                    self.source = Some(s);
                    return Err(e);
                }
                s
            }
            _ => match self.ahead.take(self.piece) {
                Some(opened) => opened?,
                None => Source::open(
                    input,
                    origin,
                    source_frame,
                    target,
                    pictures,
                    self.gpu_side().as_ref(),
                    self.decoder.as_ref(),
                    self.measures(input),
                )?,
            },
        };
        self.source = Some(source);
        self.open_next(input);
        Ok(())
    }

    /// Starts opening the file of the next source piece, unless it is
    /// `current`, which that piece then reads on with.
    fn open_next(&mut self, current: &Input) {
        let next = self
            .pieces
            .iter()
            .enumerate()
            .skip(self.piece + 1)
            .find_map(|(i, p)| match p {
                VideoPiece::Source {
                    path,
                    origin,
                    source_frame,
                    frames,
                    image,
                    ..
                } if *frames > 0 => {
                    let pictures = Pictures::of(image.as_ref());
                    let from = if pictures == Pictures::Still {
                        0
                    } else {
                        *source_frame
                    };
                    Some((i, input_of(path, image.as_ref()), *origin, from, pictures))
                }
                _ => None,
            });
        let Some((i, input, origin, source_frame, pictures)) = next else {
            return;
        };
        if &input == current {
            return;
        }
        let target = self.target(origin, source_frame);
        let gpu = self.gpu_side();
        let decoder = self.decoder.clone();
        let measure = self.measures(&input);
        let key = (input.clone(), origin.to_bits(), source_frame);
        self.ahead.start(i, key, move || {
            Source::open(
                &input,
                origin,
                source_frame,
                target,
                pictures,
                gpu.as_ref(),
                decoder.as_ref(),
                measure,
            )
        });
    }

    /// Source frames from the current piece's first to its output frame
    /// `k`, at its pace.
    fn step(&self, k: i64) -> i64 {
        self.pieces
            .get(self.piece)
            .map_or(k, |p| p.position(k) - p.position(0))
    }

    /// Moves the current source piece's file on to its output frame `k`
    /// without making the picture.
    fn pass_source_frame(&mut self, k: i64) -> Result<()> {
        let step = self.step(k);
        let first = self.source.as_ref().expect("open").source_frame;
        self.pass_at(first + step)
    }

    /// Moves the open file on to source frame `pos` without making the
    /// picture.
    fn pass_at(&mut self, pos: i64) -> Result<()> {
        let fps = self.fps;
        let src = self.source.as_mut().expect("open");
        if src.advance(target(fps, src.origin, pos))? {
            src.normalized = None;
        }
        Ok(())
    }

    /// Output frame `k` of the current source piece.
    fn source_frame(&mut self, k: i64) -> Result<frame::Video> {
        let step = self.step(k);
        let first = self.source.as_ref().expect("open").source_frame;
        self.frame_at(first + step)
    }

    /// Output frame `k` of the current piece, which walks its source
    /// backward (decision D48): made with those it shows after it, as many
    /// as fit, from the keyframe before the lowest up, so the file isn't
    /// sought for each.
    fn reversed_frame(&mut self, k: i64) -> Result<frame::Video> {
        let piece = self.pieces[self.piece].clone();
        let pos = piece.position(k);
        let input = self.source.as_ref().expect("open").reader.input().clone();
        let made = |r: &Reversed| r.pos == pos && r.input == input;
        if !self.reversed.iter().any(made) {
            self.reversed.clear();
            let most = self.reversed_frames();
            // Highest first.
            let mut shown: Vec<i64> = Vec::new();
            for j in k..piece.frames() {
                let p = piece.position(j);
                if shown.last() != Some(&p) {
                    if shown.len() as i64 == most {
                        break;
                    }
                    shown.push(p);
                }
            }
            let low = *shown.last().expect("frame k");
            for p in low..=pos {
                if shown.contains(&p) {
                    let frame = self.frame_at(p)?;
                    self.reversed.push(Reversed {
                        input: input.clone(),
                        pos: p,
                        frame,
                        dolby_vision: dolby_vision(&self.last_dolby_vision)?,
                    });
                } else {
                    self.pass_at(p)?;
                }
            }
        }
        let r = self.reversed.iter().find(|r| made(r)).expect("just made");
        self.last_dolby_vision = dolby_vision(&r.dolby_vision)?;
        frame_ref(&r.frame)
    }

    /// How many pictures [`reversed_frame`](Self::reversed_frame) makes at
    /// once: what fits in `REVERSED_BYTES`, at 10 bits.
    fn reversed_frames(&self) -> i64 {
        let current = self.source.as_ref().and_then(|s| s.current.as_ref());
        let (w, h) = match (&self.output, current) {
            (Output::Decoded { .. }, Some((f, _))) => (f.width(), f.height()),
            _ => (self.out.0, self.out.1),
        };
        let bytes = (w as usize * h as usize * 3).max(1);
        let most = if matches!(
            self.output,
            Output::Cuda { .. } | Output::FittedOnGpu { .. }
        ) {
            REVERSED_ON_GPU
        } else {
            REVERSED_MOST
        };
        ((REVERSED_BYTES / bytes) as i64).clamp(1, most)
    }

    /// The output frame of the open file's source frame `pos`.
    fn frame_at(&mut self, pos: i64) -> Result<frame::Video> {
        let (out_w, out_h, out_fmt) = self.out;
        let fps = self.fps;
        let src = self.source.as_mut().expect("open");
        let target = target(fps, src.origin, pos);
        let changed = src.advance(target)?;
        if let Output::Decoded { keep_on_gpu } = self.output {
            if changed || src.normalized.is_none() {
                let (frame, _) = src.current.as_ref().expect("have a frame");
                let relayout = &mut self.relayout;
                // Frames the GPU decoded come down to memory at their size,
                // unless a display takes them from the GPU as they are.
                src.normalized = Some(if crate::hw::on_gpu(frame) {
                    let keep =
                        keep_on_gpu && !self.unkept && displayable(crate::hw::sw_format(frame));
                    match keep.then(|| crate::hw::Keeper::keep(&mut self.keeper, frame)) {
                        Some(Ok(kept)) => kept,
                        // Down to memory, as before they were kept: a copy
                        // that failed or took two seconds would again.
                        Some(Err(e)) => {
                            eprintln!("vidcut: GPU pictures come down to be shown: {e}");
                            self.unkept = true;
                            self.keeper = None;
                            relayout.run(&crate::hw::download(frame)?)?
                        }
                        None => relayout.run(&crate::hw::download(frame)?)?,
                    }
                } else {
                    relayout.run(frame)?
                });
            }
        } else if src.on_gpu && (changed || src.normalized.is_none()) {
            src.scale_on_gpu()?;
        } else if changed || src.normalized.is_none() {
            let (frame, _) = src.current.as_ref().expect("have a frame");
            let rotation = src.reader.rotation();
            // Frames that need nothing but turning and fitting stay on the
            // GPU, for an encoder on their device.
            let on_gpu = match &self.output {
                Output::FittedOnGpu { into }
                    if crate::hw::Fitter::fits(frame, out_fmt, into.as_ref())
                        && !src.fit_failed
                        && crate::color::kept(crate::color::Source::of(frame), self.target) =>
                {
                    Some(into)
                }
                _ => None,
            };
            let tried = on_gpu.is_some();
            // A GPU that won't fit them (a driver's limits) leaves the
            // file's frames to the CPU.
            let fitted = on_gpu.and_then(|into| {
                let sar = frame.aspect_ratio();
                let sar = (sar.numerator(), sar.denominator());
                // Fitted as shown: a quarter turn swaps the sides and the
                // pixel aspect.
                let (w, h) = (frame.width(), frame.height());
                let fitted = if rotation % 180 == 90 {
                    fit(h, w, (sar.1, sar.0), out_w, out_h)
                } else {
                    fit(w, h, sar, out_w, out_h)
                };
                crate::hw::Fitter::fit(
                    &mut self.fitter,
                    frame,
                    rotation,
                    fitted,
                    (out_w, out_h),
                    out_fmt,
                    into.as_ref(),
                )
                .inspect_err(|e| {
                    crate::debug!(
                        "decoding",
                        "{}: the GPU won't fit the frames for the encoder, the CPU makes them: {e}",
                        src.reader.path().display()
                    )
                })
                .ok()
            });
            src.fit_failed |= tried && fitted.is_none();
            src.fitted = fitted.is_some();
            src.normalized = Some(match fitted {
                Some(picture) => picture,
                None => {
                    // Frames the GPU decoded come down to memory first.
                    let came_down;
                    let frame = if crate::hw::on_gpu(frame) {
                        came_down = down(&mut self.shrinker, frame, rotation, (out_w, out_h))?;
                        &came_down
                    } else {
                        frame
                    };
                    let rebuild = self
                        .normalizer
                        .as_ref()
                        .is_none_or(|n| !n.matches(frame, rotation));
                    if rebuild {
                        self.normalizer = Some(Normalizer::new(
                            frame,
                            rotation,
                            out_w,
                            out_h,
                            out_fmt,
                            self.target,
                        )?);
                    }
                    self.normalizer.as_mut().expect("built").run(frame)?
                }
            });
        }
        let picture = frame_ref(src.normalized.as_ref().expect("normalized"))?;
        self.gpu_made += i64::from(src.fitted || src.on_gpu);
        let mut out = match &self.output {
            Output::Cuda { frames, .. } if !src.on_gpu => frames.upload(&picture)?,
            _ => picture,
        };
        // The output's HDR metadata is its own: of the source's, only what
        // is kept, from the frame this one shows.
        crate::hdr::strip(&mut out);
        if self.dynamic != crate::hdr::Dynamic::default() {
            crate::hdr::copy_dynamic(&src.dynamic, &mut out, self.dynamic)?;
        }
        // Without HDR10+ of its own, made from the frame's light: measured
        // ahead with its file, or as it was decoded.
        if self.dynamic.hdr10plus
            && !crate::hdr::has_hdr10plus(&out)
            && let Some(l) = src.light.or_else(|| {
                self.light
                    .get(src.reader.path())
                    .and_then(|a| a.at(src.current.as_ref().expect("have a frame").1))
                    .copied()
            })
        {
            l.hdr10plus().attach(&mut out)?;
        }
        if self.dynamic.dolby_vision {
            self.last_dolby_vision = frame::Video::empty();
            let dv = crate::hdr::Dynamic {
                hdr10plus: false,
                dolby_vision: true,
            };
            crate::hdr::copy_dynamic(&src.dynamic, &mut self.last_dolby_vision, dv)?;
        }
        Ok(out)
    }
}

/// A frame holding only `f`'s Dolby Vision.
fn dolby_vision(f: &frame::Video) -> Result<frame::Video> {
    let mut dv = frame::Video::empty();
    let only = crate::hdr::Dynamic {
        hdr10plus: false,
        dolby_vision: true,
    };
    crate::hdr::copy_dynamic(f, &mut dv, only)?;
    Ok(dv)
}

/// Fills a frame with black: video-range for YUV, zero for RGB, opaque alpha.
pub(crate) fn fill_black(f: &mut frame::Video) -> Result<()> {
    let desc = f
        .format()
        .descriptor()
        .ok_or_else(|| Error::Invalid("unknown pixel format".into()))?;
    let (depth, shift, rgb, alpha) = unsafe {
        let d = &*desc.as_ptr();
        let flags = d.flags;
        (
            d.comp[0].depth,
            d.comp[0].shift,
            flags & (ffmpeg::ffi::AV_PIX_FMT_FLAG_RGB as u64) != 0,
            flags & (ffmpeg::ffi::AV_PIX_FMT_FLAG_ALPHA as u64) != 0,
        )
    };
    let planes = f.planes();
    for plane in 0..planes {
        let is_alpha = alpha && plane == planes - 1 && planes > 1;
        let v8: u16 = if is_alpha {
            255
        } else if rgb {
            0
        } else if plane == 0 {
            16
        } else {
            128
        };
        let data = f.data_mut(plane);
        if depth > 8 {
            // In the sample's high bits for P010 and its like.
            let v = if is_alpha {
                (1 << depth) - 1
            } else {
                v8 << (depth - 8)
            };
            for px in data.as_chunks_mut::<2>().0 {
                *px = (v << shift).to_le_bytes();
            }
        } else {
            data.fill(v8 as u8);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::fill_black;
    use crate::ffmpeg::{format::Pixel, frame};

    #[test]
    fn black_is_video_range_black_in_every_layout() {
        let sample = |pixel, plane| {
            let mut f = frame::Video::new(pixel, 16, 16);
            fill_black(&mut f).unwrap();
            let d = f.data(plane);
            u16::from_le_bytes([d[0], d[1]])
        };
        // 10 bits in the low bits, and in the high bits (P010).
        assert_eq!(sample(Pixel::YUV420P10LE, 0), 64);
        assert_eq!(sample(Pixel::YUV420P10LE, 1), 512);
        assert_eq!(sample(Pixel::P010LE, 0), 64 << 6);
        assert_eq!(sample(Pixel::P010LE, 1), 512 << 6);
        let mut f = frame::Video::new(Pixel::NV12, 16, 16);
        fill_black(&mut f).unwrap();
        assert_eq!((f.data(0)[0], f.data(1)[0]), (16, 128));
    }
}
