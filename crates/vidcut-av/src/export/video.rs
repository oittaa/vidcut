//! The video of a render plan: frames picked by the sampling rule and
//! normalized to the output size and pixel format.
//!
//! Sampling rule (docs/decisions/export.md): output frame `k` of a source piece
//! shows the first source frame whose timestamp is at least
//! `origin + (source_frame + k) / fps − ½ frame`. That is frame-exact for
//! sources at the project rate and nearest-frame sampling otherwise.

use std::path::Path;
use std::time::{Duration, Instant};

use vidcut_core::model::Hdr;
use vidcut_core::render::{Image, VideoPiece, WHOLE};
use vidcut_core::time::Rational;

use super::ahead::Ahead;
use super::make::{Fitting, Maker, Output};
use crate::decode::{Input, Reading, VideoReader};
use crate::error::{Error, Result};
use crate::ffmpeg::{self, format, frame};
use crate::route::{Place, Route};

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
    /// A still keeps its one picture and is never sought; a sequence knows
    /// how many pictures it has.
    pictures: Pictures,
    /// Its frames on the GPU, and the route that made `normalized`.
    fitting: Fitting,
    /// The GPU the reader decodes on: NVDEC for NVENC's frames
    /// ([`Output::Cuda`]), or any for frames as decoded (`decode_on`).
    decoder: Option<crate::hw::Kind>,
    /// Pictures of a piece walking its source backward, made ahead.
    reversed: Vec<Reversed>,
    /// When the pieces stopped reading it: a file left waits open a moment
    /// for a later piece of it ([`staying`]). `None` while a piece reads it.
    left: Option<Instant>,
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

/// How long a file no piece reads stays open, for a later piece of it: a
/// playhead dragged back across a cut seeks it rather than opening it anew.
const LEFT_FOR: Duration = Duration::from_secs(10);

/// Files left open at most, each holding its decoder's memory: a dissolve's
/// two. Of those a GPU decodes, one: an idle GPU decoder holds video memory.
const LEFT_MOST: usize = 2;
const LEFT_ON_GPU: usize = 1;

/// Which of the files left stay open, each given as the time since it was
/// left and whether a GPU decodes it: those left less than [`LEFT_FOR`]
/// ago, newest first, [`LEFT_MOST`] at most and [`LEFT_ON_GPU`] of a GPU's.
fn staying(left: &[(Duration, bool)]) -> Vec<bool> {
    let mut newest: Vec<usize> = (0..left.len()).collect();
    newest.sort_by_key(|&i| left[i].0);
    let mut stays = vec![false; left.len()];
    let (mut kept, mut on_gpu) = (0, 0);
    for i in newest {
        let (age, gpu) = left[i];
        if age >= LEFT_FOR || kept == LEFT_MOST || (gpu && on_gpu == LEFT_ON_GPU) {
            continue;
        }
        stays[i] = true;
        kept += 1;
        on_gpu += usize::from(gpu);
    }
    stays
}

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
    /// `reading`: how the frames are asked for ([`Reading`]). `measure`:
    /// frames without HDR10+ are measured as they are decoded.
    #[allow(clippy::too_many_arguments)]
    fn open(
        input: &Input,
        origin: f64,
        source_frame: i64,
        target: f64,
        pictures: Pictures,
        gpu: Option<&GpuSide>,
        decoder: Option<&crate::hw::GpuDecoder>,
        reading: Reading,
        measure: bool,
    ) -> Result<Source> {
        let video = pictures == Pictures::Video;
        // NVENC's pieces decode on NVDEC on a stream of their own: the next
        // piece opens, and makes its first picture, on another thread while
        // this one encodes, and frames go to the encoder once their
        // stream's work is done (`share::wait_for`).
        let nvdec = gpu.filter(|_| video);
        let nvdec = nvdec.and_then(|g| g.device.with_own_stream().ok());
        let nvdec = nvdec.map(crate::hw::GpuDecoder::nvdec).transpose()?;
        let reader =
            VideoReader::open_as(input, nvdec.as_ref().or(decoder).filter(|_| video), reading)?;
        let fitting = match gpu {
            Some(g) => Fitting::on_gpu(&reader, g.out, g.format, g.target, true, Place::Cuda),
            None => Fitting::none(),
        };
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
            pictures,
            fitting,
            decoder,
            reversed: Vec::new(),
            left: None,
        };
        if pictures == Pictures::Still {
            source.advance(target)?;
        } else {
            source.seek(target)?;
            source.advance(target)?;
        }
        // The GPU's first picture too: libplacebo compiles its shaders for
        // it, which is better done here, ahead of the piece. A frame that
        // came down after all (its deinterlacer wouldn't build) is planned
        // as it comes.
        if source.fitting.scaler.is_some()
            && let Some((frame, _)) = source.current.as_ref()
            && crate::hw::on_gpu(frame)
        {
            source.normalized = Some(source.fitting.scale(frame)?.ok_or_else(no_picture)?);
            source.fitting.took(Route::Gpu, source.reader.path());
        }
        Ok(source)
    }

    /// Opens `input` without decoding anything yet: on the GPU (`decoder`)
    /// when it decodes the stream. `reading` and `measure` as for
    /// [`open`](Self::open).
    fn bare(
        input: &Input,
        origin: f64,
        pictures: Pictures,
        decoder: Option<&crate::hw::GpuDecoder>,
        reading: Reading,
        measure: bool,
    ) -> Result<Source> {
        let reader = VideoReader::open_as(input, decoder, reading)?;
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
            pictures,
            fitting: Fitting::none(),
            decoder,
            reversed: Vec::new(),
            left: None,
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
        if !self.reads_on(target) {
            self.seek(target)?;
        }
        Ok(())
    }

    /// Whether a piece starting at `target` decodes on from where the file
    /// stands, rather than seeking: it starts a little ahead. A still is
    /// never sought.
    fn reads_on(&self, target: f64) -> bool {
        if self.pictures == Pictures::Still {
            return true;
        }
        match &self.current {
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
        }
    }

    /// No piece reads it from `now` on: it waits open, the pictures of a
    /// piece walking backward gone. The frame decoded and the picture made
    /// of it stay: making the picture may have taken the frame's buffers
    /// (a relayout's graph, the GPU's scaler).
    fn leave(&mut self, now: Instant) {
        if self.left.is_none() {
            self.left = Some(now);
            self.reversed.clear();
        }
    }

    /// Readies a file left for a piece starting at `target`, as
    /// [`open`](Self::open) leaves a file it opens: `current` at the first
    /// frame for it.
    fn resume(&mut self, origin: f64, source_frame: i64, target: f64) -> Result<()> {
        self.left = None;
        self.reposition(origin, source_frame, target)?;
        self.advance(target)?;
        Ok(())
    }
}

/// A picture of an output frame: the frame's only one, or one of the
/// layers a dissolve mixes ([`VideoFrames::next_layers`]).
pub(crate) struct Layer {
    /// `None`: black under or over the others of a mix.
    pub(crate) frame: Option<frame::Video>,
    /// Its weight over the layers under it ([`WHOLE`] is all of it);
    /// `None` for the bottom one, shown whole.
    pub(crate) weight: Option<u32>,
    /// The clockwise turn that shows it upright: its file's display matrix;
    /// 0 for black.
    pub(crate) rotation: u16,
    /// The GPU that decoded it, and what its file holds and what decoded
    /// it; `None` for black.
    pub(crate) decoder: Option<crate::hw::Kind>,
    pub(crate) decoding: Option<std::sync::Arc<crate::decode::Decoding>>,
    /// The GPU made it, for the encoder there.
    made_on_gpu: bool,
}

/// Layers of a mix at most that open files of their own while their files
/// open ahead ([`Ahead`]'s numbers count pieces this many apart).
const LAYERS: usize = 16;

/// What [`Ahead`] calls layer `layer` of piece `piece`.
fn slot(piece: usize, layer: usize) -> usize {
    piece * LAYERS + layer.min(LAYERS - 1)
}

/// The layer that shows most of a frame: from the top, each shows its
/// weight of what the ones over it leave.
fn heaviest(layers: &[Layer]) -> usize {
    let mut left = u64::from(WHOLE);
    let mut best = (0, 0);
    for (i, l) in layers.iter().enumerate().rev() {
        let w = u64::from(l.weight.unwrap_or(WHOLE));
        let shown = left * w;
        if shown > best.1 {
            best = (i, shown);
        }
        left = left * (u64::from(WHOLE) - w) / u64::from(WHOLE);
    }
    best.0
}

/// Produces the plan's video frames in order. A piece from the same file as
/// the one before it reads on with the same decoder; a piece from another
/// file is opened while the piece before it encodes. A dissolve's pictures
/// are each read from a file of their own and mixed (decision D50).
pub struct VideoFrames {
    pieces: Vec<VideoPiece>,
    fps: Rational,
    piece: usize,
    /// Frame index within the current piece.
    k: i64,
    /// The files open: those the current piece's pictures are read from,
    /// any the pieces after it may read on, and those left lately.
    open: Vec<Source>,
    /// Which of `open` reads each picture of the current piece (`None`:
    /// black).
    reading: Vec<Option<usize>>,
    /// The next file, opened ahead; named by where it opens.
    ahead: Ahead<Source, (Input, u64, i64)>,
    black: Option<frame::Video>,
    /// The GPU frames decode on ([`decode_on`](Self::decode_on)).
    decoder: Option<crate::hw::GpuDecoder>,
    /// Makes the frames into the output's pictures
    /// ([`output`](Self::output)).
    maker: Maker,
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
    /// Why mixing on NVIDIA's GPU failed: dissolves are mixed on the CPU
    /// from then on.
    cuda_mix_failed: Option<String>,
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
            piece: 0,
            k: 0,
            open: Vec::new(),
            reading: Vec::new(),
            ahead: Ahead::new(),
            black: None,
            decoder: None,
            maker: Maker::new(
                (width, height, pixel),
                target,
                Output::Fitted,
                Reading::Stream,
            ),
            gpu_made: 0,
            rotation: 0,
            last_decoder: None,
            last_decoding: None,
            gpu_decoded: 0,
            dynamic: Default::default(),
            light: Default::default(),
            last_dolby_vision: frame::Video::empty(),
            cuda_mix_failed: None,
        }
    }

    /// Why NVIDIA's GPU didn't mix the dissolves for NVENC, which the CPU
    /// mixed then; `None` where it did, or nothing was mixed there.
    pub fn mix_failed(&self) -> Option<&str> {
        self.cuda_mix_failed.as_deref()
    }

    /// Whether a dissolve's pictures are mixed on NVIDIA's GPU, for NVENC
    /// (`hw::can_mix`), rather than on the CPU.
    fn gpu_mixes(&self) -> bool {
        matches!(self.maker.output(), Output::Cuda { .. })
            && crate::hw::can_mix(Place::Cuda)
            && self.cuda_mix_failed.is_none()
    }

    /// Makes the frames into `output`. Frames are picked by the sampling
    /// rule whatever the output.
    pub(crate) fn output(mut self, output: Output) -> Self {
        let mut out = self.maker.out();
        if matches!(output, Output::Decoded { .. }) {
            out.2 = format::Pixel::YUV420P;
        }
        self.maker = Maker::new(out, self.maker.target(), output, self.maker.reading());
        self
    }

    /// Reads single frames, each after a seek ([`Reading::Frames`]): its
    /// readers and graphs start no thread.
    pub fn single_frames(mut self) -> Self {
        self.maker.read_as(Reading::Frames);
        self
    }

    /// Frames as decoded come down from the GPU shrunk to fit `size` from
    /// now on (`None`: whole, or kept there).
    pub(crate) fn shrink_to(&mut self, size: Option<(u32, u32)>) {
        if self.maker.shrinks() != size {
            self.maker.shrink_to(size);
            for s in &mut self.open {
                s.normalized = None;
            }
        }
    }

    /// Lets the maker's stages go, the GPU's converters among them: what
    /// shrinks the frames a GPU decoded ([`shrink_to`](Self::shrink_to))
    /// and what keeps them there. They are made anew as frames come.
    pub(crate) fn clear_stages(&mut self) {
        self.maker.clear_stages();
    }

    /// Frames decode on `gpu` where it decodes the stream
    /// ([`VideoReader::open_gpu_or_cpu`]): the same frames, with the same
    /// timestamps and colors. Frames as decoded ([`Output::Decoded`]) are
    /// copied down to memory at their own size; frames fitted into the
    /// output come down shrunk on the GPU to the size they are shown at,
    /// where it is smaller ([`down`](super::normalize::down)). Other
    /// streams, stills and image sequences decode on the CPU, and pieces
    /// NVDEC takes for NVENC ([`Output::Cuda`]) stay with it.
    pub fn decode_on(mut self, gpu: crate::hw::GpuDecoder) -> Self {
        self.decoder = Some(gpu);
        self
    }

    /// The GPU that decoded the last frame given (`None` for black, and for
    /// frames decoded on the CPU); of a mix, its heaviest picture's.
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
        let Output::Cuda { device, .. } = self.maker.output() else {
            return None;
        };
        let (w, h, format) = self.maker.out();
        Some(GpuSide {
            device: device.clone(),
            out: (w, h),
            format,
            target: self.maker.target(),
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

    /// The next output frame (pts unset), or `None` after the last one. A
    /// dissolve's pictures are mixed into it; not for frames as decoded
    /// ([`Output::Decoded`]), whose layers come apart
    /// ([`next_layers`](Self::next_layers)).
    pub fn next_frame(&mut self) -> Result<Option<frame::Video>> {
        let Some(layers) = self.next_layers()? else {
            return Ok(None);
        };
        if layers.len() == 1 {
            let only = layers.into_iter().next().expect("one");
            return Ok(Some(only.frame.expect("a frame's only picture")));
        }
        self.mix(layers).map(Some)
    }

    /// The next output frame's pictures, bottom up: its only one, or the
    /// layers of a dissolve's mix, each made into the output (for frames
    /// as decoded, each as decoded). `None` after the last frame.
    pub(crate) fn next_layers(&mut self) -> Result<Option<Vec<Layer>>> {
        let Some(layers) = self.next(true)? else {
            return Ok(None);
        };
        let layers = layers.expect("a picture was asked for");
        let shown = &layers[heaviest(&layers)];
        self.rotation = shown.rotation;
        self.last_decoder = shown.decoder;
        self.last_decoding = shown.decoding.clone();
        self.gpu_decoded += i64::from(shown.decoder.is_some());
        let made_on_gpu = layers
            .iter()
            .all(|l| l.made_on_gpu || (l.frame.is_none() && layers.len() > 1));
        self.gpu_made += i64::from(made_on_gpu);
        Ok(Some(layers))
    }

    /// Moves past the next output frame without making its picture: it is
    /// decoded, since decoding on needs it, but not scaled. Returns whether
    /// there was a frame.
    pub fn skip_frame(&mut self) -> Result<bool> {
        Ok(self.next(false)?.is_some())
    }

    /// The next output frame, with its pictures when `picture` is set.
    fn next(&mut self, picture: bool) -> Result<Option<Option<Vec<Layer>>>> {
        loop {
            let Some(piece) = self.pieces.get(self.piece) else {
                return Ok(None);
            };
            if self.k >= piece.frames() {
                // The files stay open: a later piece may read them too.
                self.piece += 1;
                self.k = 0;
                continue;
            }
            let k = self.k;
            self.k += 1;
            let piece = piece.clone();
            let layers = piece.layers();
            let files = layers
                .iter()
                .any(|(l, _)| matches!(l, VideoPiece::Source { .. }));
            if k == 0 && files {
                self.start_layers(&piece)?;
            }
            if k == 1 && files {
                // The next file opens ahead once the piece plays on: a
                // seek a frame (a dragged playhead) opens nothing ahead.
                self.open_next();
            }
            let mixed = layers.len() > 1;
            self.maker.set_mixing(mixed);
            let mut made = Vec::with_capacity(layers.len());
            for (i, (layer, ramp)) in layers.into_iter().enumerate() {
                let weight = ramp.map(|r| r.weight(k));
                let VideoPiece::Source { image, pace, .. } = layer else {
                    if picture {
                        // Frames as decoded hand black on as a picture.
                        let decoded = matches!(self.maker.output(), Output::Decoded { .. });
                        let frame = if mixed && !decoded {
                            None
                        } else {
                            Some(self.black_frame()?)
                        };
                        made.push(Layer {
                            frame,
                            weight,
                            rotation: 0,
                            decoder: None,
                            decoding: None,
                            made_on_gpu: false,
                        });
                    }
                    continue;
                };
                // A still's one picture serves every frame: it is never
                // sought (an image file has nothing past it).
                let still = Pictures::of(image.as_ref()) == Pictures::Still;
                let backward = pace.speed < 0.0 && !still;
                let k = if still { 0 } else { k };
                let s = self.reading[i].expect("a file's picture is read");
                if !picture {
                    // Made when one is asked for, from a keyframe.
                    if !backward {
                        self.pass_source_frame(s, layer, k)?;
                    }
                    continue;
                }
                let (rotation, decoder, decoding) = {
                    let src = &self.open[s];
                    (
                        src.reader.rotation(),
                        src.decoder,
                        src.reader.decoding().clone(),
                    )
                };
                let frame = if backward {
                    self.reversed_frame(i, s, layer, k)?
                } else {
                    self.source_frame(i, s, layer, k)?
                };
                made.push(Layer {
                    frame: Some(frame),
                    weight,
                    rotation,
                    decoder,
                    decoding: Some(decoding),
                    made_on_gpu: self.open[s].fitting.route == Some(Route::Gpu),
                });
            }
            return Ok(Some(picture.then_some(made)));
        }
    }

    /// The output's black, for a frame of black alone: with HDR10+ of no
    /// light, and the Dolby Vision of the source frame before it.
    fn black_frame(&mut self) -> Result<frame::Video> {
        let black = frame_ref(self.black()?)?;
        let mut out = match self.maker.output() {
            Output::Cuda { frames, .. } => frames.upload(&black)?,
            _ => black,
        };
        self.black_metadata(&mut out)?;
        Ok(out)
    }

    /// Black's dynamic HDR metadata on `out`: HDR10+ of no light, and the
    /// Dolby Vision of the source frame before it.
    fn black_metadata(&self, out: &mut frame::Video) -> Result<()> {
        if self.dynamic.hdr10plus {
            crate::light::FrameLight::default()
                .hdr10plus()
                .attach(out)?;
        }
        if self.dynamic.dolby_vision {
            let dv = crate::hdr::Dynamic {
                hdr10plus: false,
                dolby_vision: true,
            };
            crate::hdr::copy_dynamic(&self.last_dolby_vision, out, dv)?;
        }
        Ok(())
    }

    /// `layers` mixed into one picture, each over those under it at its
    /// weight (decision D50): on NVIDIA's GPU for NVENC, else on the CPU.
    /// The picture keeps the colors and dynamic HDR metadata of the layer
    /// that shows most of it.
    fn mix(&mut self, layers: Vec<Layer>) -> Result<frame::Video> {
        if let Output::Decoded { .. } = self.maker.output() {
            return Err(Error::Invalid(
                "pictures as decoded come apart, not mixed".into(),
            ));
        }
        let shown = heaviest(&layers);
        let on_gpu = match self.maker.output() {
            Output::Cuda { frames, .. } if self.gpu_mixes() => match cuda_mix(frames, &layers) {
                Ok(mixed) => Some(mixed),
                // A GPU that won't mix (its driver won't take the kernel)
                // leaves the export's dissolves to the CPU.
                Err(e) => {
                    crate::debug!(
                        "decoding",
                        "the GPU won't mix a dissolve's pictures, the CPU mixes them: {e}"
                    );
                    self.cuda_mix_failed = Some(e.to_string());
                    None
                }
            },
            _ => None,
        };
        let mut out = match on_gpu {
            Some(mixed) => mixed,
            None => {
                let black = frame_ref(self.black()?)?;
                let in_memory = |f: Option<&frame::Video>| -> Result<frame::Video> {
                    match f {
                        None => frame_ref(&black),
                        Some(f) if Place::of(f) == Place::Memory => frame_ref(f),
                        Some(f) => crate::hw::download(f),
                    }
                };
                let mut mixed = in_memory(layers[0].frame.as_ref())?;
                for l in &layers[1..] {
                    let over = in_memory(l.frame.as_ref())?;
                    mixed = super::mix::mix(&mixed, &over, l.weight.unwrap_or(WHOLE))?;
                }
                match self.maker.output() {
                    // Up to NVENC's frames, mixed.
                    Output::Cuda { frames, .. } => frames.upload(&mixed)?,
                    _ => mixed,
                }
            }
        };
        // The colors of the picture it mostly shows, or of any.
        let like = layers[shown]
            .frame
            .as_ref()
            .or_else(|| layers.iter().find_map(|l| l.frame.as_ref()));
        if let Some(like) = like {
            // SAFETY: both are valid frames.
            unsafe { ffmpeg::ffi::av_frame_copy_props(out.as_mut_ptr(), like.as_ptr()) };
        }
        crate::hdr::strip(&mut out);
        // Dolby Vision of the picture it shows most of (black's: that of the
        // frame before it).
        if self.dynamic.dolby_vision {
            let dv = crate::hdr::Dynamic {
                hdr10plus: false,
                dolby_vision: true,
            };
            let from = layers[shown]
                .frame
                .as_ref()
                .unwrap_or(&self.last_dolby_vision);
            crate::hdr::copy_dynamic(from, &mut out, dv)?;
        }
        // HDR10+ never claims less light than it shows: each value the
        // brightest picture's. Black alone has no light; pictures without
        // HDR10+ leave the mix without it, as they are outside one.
        if self.dynamic.hdr10plus {
            let light = layers
                .iter()
                .filter_map(|l| l.frame.as_ref())
                .filter_map(crate::hdr::Hdr10Plus::of_frame)
                .reduce(|a, b| a.brightest(&b));
            let black = layers.iter().all(|l| l.frame.is_none());
            match light {
                Some(light) => light.attach(&mut out)?,
                None if black => crate::light::FrameLight::default()
                    .hdr10plus()
                    .attach(&mut out)?,
                None => {}
            }
        }
        Ok(out)
    }

    /// Forgets where the open files stand, so the next piece of each seeks
    /// rather than decoding on: after decoding was interrupted (a cancel
    /// flag), a reader may stand anywhere. A still keeps its picture.
    pub fn forget_position(&mut self) {
        for s in self
            .open
            .iter_mut()
            .filter(|s| s.pictures != Pictures::Still)
        {
            s.current = None;
            s.normalized = None;
        }
    }

    /// The time (seconds) of the keyframe decoding starts from for the
    /// first frame of `piece`, a video file's: a seek and one decoded frame.
    /// The reader stays there: frames read from the keyframe on decode on
    /// without seeking again. `None` for black, stills, image sequences and
    /// mixes.
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
        let s = match self.open.iter().position(|s| s.reader.input() == &input) {
            Some(s) => s,
            None => {
                let bare = Source::bare(
                    &input,
                    *origin,
                    Pictures::Video,
                    self.decoder.as_ref(),
                    self.maker.reading(),
                    self.measures(&input),
                )?;
                self.open.push(bare);
                self.open.len() - 1
            }
        };
        let source = &mut self.open[s];
        // Interrupted, the file stays open, its position unknown.
        let sought = source.seek(target);
        // The window that starts there decodes on from it.
        source.reader.from_keyframe();
        let at = source.reader.pending_pts().map(|p| p as f64 * source.tick);
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
            let (w, h, pix) = self.maker.out();
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

    /// Readies the files of the current piece's pictures: for each, an open
    /// file of it no other picture of the piece reads (the one that reads on
    /// to it, if one does; a file left lately among them), the one opened
    /// ahead for it, or a new one. Files none of them reads are left, and
    /// those left that wait no more close ([`close_left`](Self::close_left)).
    fn start_layers(&mut self, piece: &VideoPiece) -> Result<()> {
        let mut taken = vec![false; self.open.len()];
        let mut reading = Vec::new();
        for (i, (layer, _)) in piece.layers().into_iter().enumerate() {
            let VideoPiece::Source {
                path,
                origin,
                source_frame,
                image,
                pace,
                ..
            } = layer
            else {
                reading.push(None);
                continue;
            };
            let pictures = Pictures::of(image.as_ref());
            let still = pictures == Pictures::Still;
            let from = if still { 0 } else { *source_frame };
            let input = input_of(path, image.as_ref());
            let target = self.target(*origin, from);
            let free: Vec<usize> = (0..self.open.len())
                .filter(|&s| !taken[s] && self.open[s].reader.input() == &input)
                .collect();
            let reads_on = free
                .iter()
                .copied()
                .find(|&s| self.open[s].reads_on(target));
            let s = match reads_on.or(free.first().copied()) {
                Some(s) => {
                    if let Err(e) = self.open[s].reposition(*origin, from, target) {
                        // Interrupted, say: the file stays open, its
                        // position unknown.
                        self.open[s].current = None;
                        self.open[s].normalized = None;
                        return Err(e);
                    }
                    s
                }
                None => {
                    let opened = match self.ahead.take(slot(self.piece, i)) {
                        Some(opened) => opened?,
                        None => Source::open(
                            &input,
                            *origin,
                            from,
                            target,
                            pictures,
                            self.gpu_side().as_ref(),
                            self.decoder.as_ref(),
                            self.maker.reading(),
                            self.measures(&input),
                        )?,
                    };
                    self.open.push(opened);
                    taken.push(false);
                    self.open.len() - 1
                }
            };
            taken[s] = true;
            if !(pace.speed < 0.0 && !still) {
                self.open[s].reversed.clear();
            }
            reading.push(Some(s));
        }
        let now = Instant::now();
        for (source, taken) in self.open.iter_mut().zip(taken) {
            if taken {
                source.left = None;
            } else {
                source.leave(now);
            }
        }
        self.reading = reading;
        self.close_left();
        Ok(())
    }

    /// Closes the files left that wait no more ([`staying`]): left
    /// [`LEFT_FOR`] ago, or the oldest past [`LEFT_MOST`] (of a GPU's,
    /// [`LEFT_ON_GPU`]). On the thread that reads, as a piece starts and
    /// whenever the reader idles.
    pub(crate) fn close_left(&mut self) {
        let now = Instant::now();
        let left: Vec<usize> = (0..self.open.len())
            .filter(|&s| self.open[s].left.is_some())
            .collect();
        let ages: Vec<(Duration, bool)> = left
            .iter()
            .map(|&s| {
                let source = &self.open[s];
                let since = source.left.expect("left");
                (now.duration_since(since), source.decoder.is_some())
            })
            .collect();
        let stays = staying(&ages);
        let mut closed = false;
        for (&s, stays) in left.iter().zip(stays).rev() {
            if !stays {
                drop(self.take_out(s));
                closed = true;
            }
        }
        if closed {
            // Their decoders' memory goes back.
            crate::buffers::give_back();
        }
    }

    /// Open file `s`, one the current piece doesn't read, out of the files
    /// open: the current piece's pictures read on from those after it, one
    /// place down.
    fn take_out(&mut self, s: usize) -> Source {
        for r in self.reading.iter_mut().flatten() {
            if *r > s {
                *r -= 1;
            }
        }
        self.open.remove(s)
    }

    /// Starts opening the file of the next piece's first picture that no
    /// open file reads; the next piece reads on with the open ones. A file
    /// left lately isn't opened anew: it goes to the opener, which readies
    /// it there.
    fn open_next(&mut self) {
        let next = self
            .pieces
            .iter()
            .enumerate()
            .skip(self.piece + 1)
            .find(|(_, p)| {
                p.layers()
                    .iter()
                    .any(|(l, _)| matches!(l, VideoPiece::Source { frames, .. } if *frames > 0))
            })
            .map(|(j, p)| (j, p.clone()));
        let Some((j, piece)) = next else {
            return;
        };
        let mut used = vec![false; self.open.len()];
        for (i, (layer, _)) in piece.layers().into_iter().enumerate() {
            let VideoPiece::Source {
                path,
                origin,
                source_frame,
                image,
                ..
            } = layer
            else {
                continue;
            };
            let input = input_of(path, image.as_ref());
            let free = |s: &usize| !used[*s] && self.open[*s].reader.input() == &input;
            let read = (0..self.open.len()).find(|s| free(s) && self.open[*s].left.is_none());
            let left = (0..self.open.len()).find(free);
            if let Some(s) = read {
                used[s] = true;
                continue;
            }
            let pictures = Pictures::of(image.as_ref());
            let from = if pictures == Pictures::Still {
                0
            } else {
                *source_frame
            };
            let origin = *origin;
            let target = self.target(origin, from);
            let gpu = self.gpu_side();
            let decoder = self.decoder.clone();
            let measure = self.measures(&input);
            let reading = self.maker.reading();
            let key = (input.clone(), origin.to_bits(), from);
            if let Some(s) = left {
                let mut source = self.take_out(s);
                self.ahead.start(slot(j, i), key, move || {
                    source.resume(origin, from, target)?;
                    Ok(source)
                });
                return;
            }
            self.ahead.start(slot(j, i), key, move || {
                Source::open(
                    &input,
                    origin,
                    from,
                    target,
                    pictures,
                    gpu.as_ref(),
                    decoder.as_ref(),
                    reading,
                    measure,
                )
            });
            return;
        }
    }

    /// Source frames from `layer`'s first to its output frame `k`, at its
    /// pace.
    fn step(layer: &VideoPiece, k: i64) -> i64 {
        layer.position(k) - layer.position(0)
    }

    /// Moves open file `s` on to `layer`'s output frame `k` without making
    /// the picture.
    fn pass_source_frame(&mut self, s: usize, layer: &VideoPiece, k: i64) -> Result<()> {
        let first = self.open[s].source_frame;
        self.pass_at(s, first + Self::step(layer, k))
    }

    /// Moves open file `s` on to source frame `pos` without making the
    /// picture.
    fn pass_at(&mut self, s: usize, pos: i64) -> Result<()> {
        let fps = self.fps;
        let src = &mut self.open[s];
        if src.advance(target(fps, src.origin, pos))? {
            src.normalized = None;
        }
        Ok(())
    }

    /// Output frame `k` of `layer`, picture `i` of the current piece, read
    /// from open file `s`.
    fn source_frame(
        &mut self,
        i: usize,
        s: usize,
        layer: &VideoPiece,
        k: i64,
    ) -> Result<frame::Video> {
        let first = self.open[s].source_frame;
        self.frame_at(i, s, first + Self::step(layer, k))
    }

    /// Output frame `k` of `layer`, picture `i` of the current piece, which
    /// walks its source backward (decision D48), read from open file `s`:
    /// made with those it shows after it, as many as fit, from the keyframe
    /// before the lowest up, so the file isn't sought for each.
    fn reversed_frame(
        &mut self,
        i: usize,
        s: usize,
        layer: &VideoPiece,
        k: i64,
    ) -> Result<frame::Video> {
        let pos = layer.position(k);
        let input = self.open[s].reader.input().clone();
        let made = |r: &Reversed| r.pos == pos && r.input == input;
        if !self.open[s].reversed.iter().any(made) {
            self.open[s].reversed.clear();
            let most = self.reversed_frames(s);
            // Highest first.
            let mut shown: Vec<i64> = Vec::new();
            for j in k..layer.frames() {
                let p = layer.position(j);
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
                    let frame = self.frame_at(i, s, p)?;
                    let dolby_vision = dolby_vision(&self.last_dolby_vision)?;
                    self.open[s].reversed.push(Reversed {
                        input: input.clone(),
                        pos: p,
                        frame,
                        dolby_vision,
                    });
                } else {
                    self.pass_at(s, p)?;
                }
            }
        }
        let r = self.open[s]
            .reversed
            .iter()
            .find(|r| made(r))
            .expect("just made");
        self.last_dolby_vision = dolby_vision(&r.dolby_vision)?;
        frame_ref(&r.frame)
    }

    /// How many pictures [`reversed_frame`](Self::reversed_frame) makes at
    /// once from open file `s`: what fits in `REVERSED_BYTES`, at 10 bits.
    fn reversed_frames(&self, s: usize) -> i64 {
        let current = self.open[s].current.as_ref();
        let (w, h) = match (self.maker.output(), current) {
            (Output::Decoded { .. }, Some((f, _))) => (f.width(), f.height()),
            _ => (self.maker.out().0, self.maker.out().1),
        };
        let bytes = (w as usize * h as usize * 3).max(1);
        let most = if matches!(
            self.maker.output(),
            Output::Cuda { .. } | Output::FittedOnGpu { .. }
        ) {
            REVERSED_ON_GPU
        } else {
            REVERSED_MOST
        };
        ((REVERSED_BYTES / bytes) as i64).clamp(1, most)
    }

    /// The output picture `i` of the current piece shows, of open file
    /// `s`'s source frame `pos`.
    fn frame_at(&mut self, i: usize, s: usize, pos: i64) -> Result<frame::Video> {
        let fps = self.fps;
        // Pictures made in memory go up to NVENC's frames, but for a mix
        // the CPU makes.
        let up = !self.maker.mixing() || self.gpu_mixes();
        let src = &mut self.open[s];
        let changed = src.advance(target(fps, src.origin, pos))?;
        if changed || src.normalized.is_none() {
            let (frame, _) = src.current.as_ref().expect("have a frame");
            let rotation = src.reader.rotation();
            let made =
                self.maker
                    .make_layer(i, &mut src.fitting, frame, rotation, src.reader.path())?;
            src.normalized = Some(made.ok_or_else(no_picture)?);
        }
        let picture = frame_ref(src.normalized.as_ref().expect("normalized"))?;
        let mut out = match self.maker.output() {
            // Pictures made in memory go up to NVENC's frames.
            Output::Cuda { frames, .. } if up && Place::of(&picture) == Place::Memory => {
                frames.upload(&picture)?
            }
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

/// `layers`, frames of `frames`' size and layout in CUDA memory (`None`:
/// black), mixed on NVIDIA's GPU: each over those under it at its weight.
fn cuda_mix(frames: &crate::cuda::CudaFrames, layers: &[Layer]) -> Result<frame::Video> {
    let mut mixed: Option<frame::Video> = None;
    for l in layers {
        match (mixed.take(), l.weight) {
            (None, None) => mixed = l.frame.as_ref().map(frame_ref).transpose()?,
            (under, w) => {
                let w = w.unwrap_or(WHOLE);
                mixed = Some(crate::cuda::mix::mix(
                    frames,
                    under.as_ref(),
                    l.frame.as_ref(),
                    w,
                )?);
            }
        }
    }
    mixed.ok_or_else(|| Error::Invalid("a mix of black alone".into()))
}

/// What a scaler that gave no picture for a frame says: an export's gives
/// each frame's at once.
fn no_picture() -> Error {
    Error::Invalid("the GPU scaler gave no picture".into())
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
    // Packed with an alpha component (RGBA): one plane, opaque below.
    let packed_alpha = alpha && planes == 1;
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
    if packed_alpha {
        // SAFETY: FFmpeg's static descriptor of the format.
        let (step, offset, a_depth) = unsafe {
            let d = &*desc.as_ptr();
            let a = d.comp[usize::from(d.nb_components) - 1];
            (a.step as usize, a.offset as usize, a.depth)
        };
        let (w, h, stride) = (f.width() as usize, f.height() as usize, f.stride(0));
        let data = f.data_mut(0);
        for y in 0..h {
            for x in 0..w {
                let at = y * stride + x * step + offset;
                if a_depth > 8 {
                    data[at..at + 2].copy_from_slice(&u16::MAX.to_le_bytes());
                } else {
                    data[at] = 255;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{fill_black, staying};
    use crate::ffmpeg::{format::Pixel, frame};

    const fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn a_file_left_closes_ten_seconds_after() {
        assert_eq!(
            staying(&[(secs(9), false), (secs(10), false)]),
            [true, false]
        );
    }

    #[test]
    fn past_two_files_left_the_oldest_closes() {
        assert_eq!(
            staying(&[(secs(3), false), (secs(1), false), (secs(2), false)]),
            [false, true, true]
        );
    }

    #[test]
    fn one_file_left_a_gpu_decodes_stays() {
        assert_eq!(
            staying(&[(secs(2), true), (secs(1), true), (secs(3), false)]),
            [false, true, true]
        );
    }

    #[test]
    fn packed_rgba_black_is_opaque() {
        let mut f = frame::Video::new(Pixel::RGBA, 4, 4);
        fill_black(&mut f).unwrap();
        assert_eq!(&f.data(0)[..4], [0, 0, 0, 255]);
    }

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
