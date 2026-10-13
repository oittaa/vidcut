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
use crate::decode::{Input, VideoReader};
use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, filter, format, frame};

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

/// Turns pictures upright (the stream's rotation), scales them keeping the
/// display aspect (honoring the sample aspect ratio), pads to the output
/// size, and converts them to the output's colors and pixel format.
pub(crate) struct Normalizer {
    graph: filter::Graph,
    /// Input parameters the graph was built for, and the rotation.
    input: (
        u32,
        u32,
        format::Pixel,
        (i32, i32),
        u16,
        crate::color::Source,
    ),
}

/// The filters that turn a picture stored with clockwise `rotation` upright.
fn upright(rotation: u16) -> &'static str {
    match rotation {
        90 => "transpose=clock,",
        180 => "hflip,vflip,",
        270 => "transpose=cclock,",
        _ => "",
    }
}

/// The scaled size of a `w`×`h` picture with sample aspect `sar` fitted inside
/// `out_w`×`out_h`, rounded to even numbers.
pub(crate) fn fit(w: u32, h: u32, sar: (i32, i32), out_w: u32, out_h: u32) -> (u32, u32) {
    let sar = if sar.0 > 0 && sar.1 > 0 {
        f64::from(sar.0) / f64::from(sar.1)
    } else {
        1.0
    };
    let dar = f64::from(w) * sar / f64::from(h);
    let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
    if dar > f64::from(out_w) / f64::from(out_h) {
        (out_w, even(f64::from(out_w) / dar).min(out_h))
    } else {
        (even(f64::from(out_h) * dar).min(out_w), out_h)
    }
}

impl Normalizer {
    /// `target`: the output's dynamic range (`None`: SDR BT.709).
    pub(crate) fn new(
        src: &frame::Video,
        rotation: u16,
        out_w: u32,
        out_h: u32,
        out_fmt: format::Pixel,
        target: Option<Hdr>,
    ) -> Result<Self> {
        let (w, h, fmt) = (src.width(), src.height(), src.format());
        let sar = src.aspect_ratio();
        let sar = (sar.numerator(), sar.denominator());
        // Fitted as shown: a quarter turn swaps the sides and the pixel aspect.
        let (sw, sh) = if rotation % 180 == 90 {
            fit(h, w, (sar.1, sar.0), out_w, out_h)
        } else {
            fit(w, h, sar, out_w, out_h)
        };
        let fmt_name = |p: format::Pixel| {
            p.descriptor()
                .map(|d| d.name().to_string())
                .unwrap_or_default()
        };
        let mut graph = filter::Graph::new();
        let args = format!(
            "video_size={w}x{h}:pix_fmt={}:time_base=1/1000000:pixel_aspect={}/{}",
            ffmpeg::ffi::AVPixelFormat::from(fmt) as i32,
            sar.0.max(1),
            sar.1.max(1)
        );
        graph
            .add(
                &filter::find("buffer").ok_or_else(|| Error::Invalid("no buffer filter".into()))?,
                "in",
                &args,
            )
            .ctx(|| "creating video source".into())?;
        graph
            .add(
                &filter::find("buffersink")
                    .ok_or_else(|| Error::Invalid("no buffersink filter".into()))?,
                "out",
                "",
            )
            .ctx(|| "creating video sink".into())?;
        let color = crate::color::Source::of(src);
        let conv = crate::color::conversion(color, out_fmt, target);
        let spec = format!(
            "{}scale={sw}:{sh}:flags=bicubic{},pad={out_w}:{out_h}:{}:{}:color=black,setsar=1{},format={}",
            upright(rotation),
            conv.scale,
            (out_w - sw) / 2,
            (out_h - sh) / 2,
            conv.after,
            fmt_name(out_fmt)
        );
        graph
            .output("in", 0)
            .and_then(|p| p.input("out", 0))
            .and_then(|p| p.parse(&spec))
            .ctx(|| format!("building filter {spec}"))?;
        graph
            .validate()
            .ctx(|| format!("configuring filter {spec}"))?;
        Ok(Self {
            graph,
            input: (w, h, fmt, sar, rotation, color),
        })
    }

    pub(crate) fn matches(&self, src: &frame::Video, rotation: u16) -> bool {
        let sar = src.aspect_ratio();
        self.input
            == (
                src.width(),
                src.height(),
                src.format(),
                (sar.numerator(), sar.denominator()),
                rotation,
                crate::color::Source::of(src),
            )
    }

    pub(crate) fn run(&mut self, src: &frame::Video) -> Result<frame::Video> {
        let mut input = frame_ref(src)?;
        input.set_pts(Some(0));
        self.graph
            .get("in")
            .expect("source")
            .source()
            .add(&input)
            .ctx(|| "filtering frame".into())?;
        let mut out = frame::Video::empty();
        self.graph
            .get("out")
            .expect("sink")
            .sink()
            .frame(&mut out)
            .ctx(|| "filtering frame".into())?;
        Ok(out)
    }
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

/// `input` opened on NVDEC, and the scaler to the output, when the scaler
/// builds (libplacebo needs Vulkan for turned pictures, other matrices than
/// BT.709, and HDR). The reader deinterlaces interlaced frames on the GPU
/// in decode order, before the sampling rule picks them, as on the CPU.
fn gpu_reader(input: &Input, gpu: &GpuSide) -> Option<(VideoReader, crate::cuda::CudaPictures)> {
    // A stream of its own: the next piece opens, and makes its first
    // picture, on another thread while this one encodes. Frames go to the
    // encoder once their stream's work is done (`share::wait_for`).
    let device = gpu.device.with_own_stream().ok()?;
    let reader = VideoReader::open_cuda(input, &device).ok()?;
    let scaler =
        crate::cuda::CudaPictures::new(&device, &reader, gpu.out, gpu.format, gpu.target, true)
            .ok()?;
    Some((reader, scaler))
}

/// How far past the current frame (seconds) a later piece of the same file
/// may start for decoding on to it to beat a seek.
const DECODE_AHEAD: f64 = 1.0;

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
        let on_gpu = gpu.filter(|_| video).and_then(|g| gpu_reader(input, g));
        let (reader, scaler) = match on_gpu {
            Some((r, s)) => (r, Some(s)),
            // Frames as decoded: on the GPU when it decodes the stream.
            None => match decoder.filter(|_| video) {
                Some(d) => (VideoReader::open_gpu_or_cpu(input, d)?, None),
                None => (VideoReader::open_input(input)?, None),
            },
        };
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
    normalizer: Option<Normalizer>,
    black: Option<frame::Video>,
    /// The output's dynamic range (`None`: SDR BT.709).
    target: Option<Hdr>,
    /// Frames for NVENC in GPU memory (`with_gpu`).
    gpu: Option<Gpu>,
    /// Frames as decoded (`decoded`): each picture's own size and colors,
    /// in a layout a display takes.
    decoded: Option<Relayout>,
    /// Frames as decoded come from the GPU where it decodes the stream, and
    /// are copied down from it (`decode_on`) unless they stay there
    /// (`keep_on_gpu`).
    decoder: Option<crate::hw::GpuDecoder>,
    keep_on_gpu: bool,
    /// Copies VA-API's frames kept on the GPU out of the decoder's surfaces.
    keeper: Option<crate::hw::Keeper>,
    /// The clockwise turn that shows the last frame given upright.
    rotation: u16,
    /// The GPU that decoded the last frame given.
    last_decoder: Option<crate::hw::Kind>,
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
}

/// The layouts a display takes: 8-bit 4:2:0 (planar, or NV12 as hardware
/// decoders give it) and P010 for more bits.
pub(crate) fn displayable(p: format::Pixel) -> bool {
    use format::Pixel as P;
    matches!(p, P::YUV420P | P::YUVJ420P | P::NV12 | P::P010LE)
}

/// Brings decoded pictures into a layout a display takes, at their own size
/// and in their own colors: 4:2:2, 4:4:4 and 10-bit planar pictures are
/// resampled to 4:2:0 (8 bits, or P010 for more), keeping their matrix and
/// range; RGB pictures (images) are coded as BT.709 video range. Pictures
/// already in such a layout pass as they are, by reference.
pub(crate) struct Relayout {
    graph: Option<(filter::Graph, (u32, u32, format::Pixel))>,
}

impl Relayout {
    pub(crate) fn new() -> Self {
        Self { graph: None }
    }

    pub(crate) fn run(&mut self, src: &frame::Video) -> Result<frame::Video> {
        let fmt = src.format();
        if displayable(fmt) {
            return frame_ref(src);
        }
        let key = (src.width(), src.height(), fmt);
        if self.graph.as_ref().is_none_or(|(_, k)| *k != key) {
            self.graph = Some((Self::build(src)?, key));
        }
        let (graph, _) = self.graph.as_mut().expect("built");
        graph
            .get("in")
            .expect("source")
            .source()
            .add(src)
            .ctx(|| "converting a picture's layout".into())?;
        let mut out = frame::Video::empty();
        graph
            .get("out")
            .expect("sink")
            .sink()
            .frame(&mut out)
            .ctx(|| "converting a picture's layout".into())?;
        Ok(out)
    }

    fn build(src: &frame::Video) -> Result<filter::Graph> {
        let desc = src
            .format()
            .descriptor()
            .ok_or_else(|| Error::Invalid("unknown pixel format".into()))?;
        let (depth, rgb) = unsafe {
            let d = &*desc.as_ptr();
            (
                d.comp[0].depth,
                d.flags & (ffmpeg::ffi::AV_PIX_FMT_FLAG_RGB as u64) != 0,
            )
        };
        let target = if depth > 8 { "p010le" } else { "yuv420p" };
        // A YUV picture keeps its matrix and range, named on both sides:
        // told nothing, swscale would convert to BT.601.
        let spec = if rgb {
            format!("scale=out_color_matrix=bt709:out_range=limited,format={target}")
        } else {
            format!("scale={},format={target}", crate::color::keep(src))
        };
        let sar = src.aspect_ratio();
        let args = format!(
            "video_size={}x{}:pix_fmt={}:time_base=1/1000000:pixel_aspect={}/{}",
            src.width(),
            src.height(),
            ffmpeg::ffi::AVPixelFormat::from(src.format()) as i32,
            sar.numerator().max(1),
            sar.denominator().max(1)
        );
        let find = |name: &str| {
            filter::find(name).ok_or_else(|| Error::Invalid(format!("no {name} filter")))
        };
        let mut graph = filter::Graph::new();
        graph
            .add(&find("buffer")?, "in", &args)
            .ctx(|| "creating video source".into())?;
        graph
            .add(&find("buffersink")?, "out", "")
            .ctx(|| "creating video sink".into())?;
        graph
            .output("in", 0)
            .and_then(|p| p.input("out", 0))
            .and_then(|p| p.parse(&spec))
            .ctx(|| format!("building filter {spec}"))?;
        graph
            .validate()
            .ctx(|| format!("configuring filter {spec}"))?;
        Ok(graph)
    }
}

/// The GPU side of an export whose frames stay there.
struct Gpu {
    side: GpuSide,
    /// Where pictures made on the CPU, black among them, go up.
    frames: crate::cuda::CudaFrames,
    /// Frames decoded and scaled on the GPU so far.
    made: i64,
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
            normalizer: None,
            black: None,
            target,
            gpu: None,
            decoded: None,
            decoder: None,
            keep_on_gpu: false,
            keeper: None,
            rotation: 0,
            last_decoder: None,
            dynamic: Default::default(),
            light: Default::default(),
            last_dolby_vision: frame::Video::empty(),
        }
    }

    /// Gives frames as decoded instead of fitted into the output: each at
    /// its own size, in its own colors, upright only after
    /// [`rotation`](Self::rotation), in a layout a display takes (8-bit
    /// 4:2:0 or P010; see `Relayout`). Frames are still picked by the
    /// sampling rule. Black is the output's size, in 8-bit 4:2:0.
    pub fn decoded(mut self) -> Self {
        self.decoded = Some(Relayout::new());
        self.out.2 = format::Pixel::YUV420P;
        self
    }

    /// Frames as decoded ([`decoded`](Self::decoded)) decode on `gpu` where
    /// it decodes the stream ([`VideoReader::open_gpu_or_cpu`]), and are
    /// copied down to memory at their own size: the same frames, with the
    /// same timestamps and colors. Other streams, stills and image
    /// sequences decode on the CPU.
    pub fn decode_on(mut self, gpu: crate::hw::GpuDecoder) -> Self {
        self.decoder = Some(gpu);
        self
    }

    /// Frames NVDEC or VA-API decoded ([`decode_on`](Self::decode_on)) stay
    /// in GPU memory when a display takes them as they are (NV12, P010): for
    /// a display that shows them from there. VA-API's are copied out of the
    /// decoder's surfaces on the GPU ([`crate::hw::Keeper`]). Others, and
    /// other GPUs', are copied down.
    pub fn keep_on_gpu(mut self) -> Self {
        self.keep_on_gpu = true;
        self
    }

    /// The GPU that decoded the last frame given (`None` for black, and for
    /// frames decoded on the CPU).
    pub fn decoded_on(&self) -> Option<crate::hw::Kind> {
        self.last_decoder
    }

    /// The clockwise turn (0, 90, 180 or 270) that shows the last frame
    /// given upright: its file's display matrix; 0 for black.
    pub fn rotation(&self) -> u16 {
        self.rotation
    }

    /// Every frame in GPU memory, from `frames` (made for this output's
    /// size and format, which must be NV12 or P010): pieces the GPU shows
    /// as the CPU would decode on NVDEC and scale there; the rest, and
    /// black, are made on the CPU and go up.
    pub fn with_gpu(
        mut self,
        device: crate::cuda::CudaDevice,
        frames: crate::cuda::CudaFrames,
    ) -> Self {
        self.gpu = Some(Gpu {
            side: GpuSide {
                device,
                out: (self.out.0, self.out.1),
                format: self.out.2,
                target: self.target,
            },
            frames,
            made: 0,
        });
        self
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

    /// Frames decoded and scaled on the GPU so far.
    pub fn gpu_frames(&self) -> i64 {
        self.gpu.as_ref().map_or(0, |g| g.made)
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
                    let black = frame_ref(self.black()?)?;
                    let mut out = match &self.gpu {
                        Some(g) => g.frames.upload(&black)?,
                        None => black,
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
                    ..
                } => {
                    // A still's one picture serves every frame: it is never
                    // sought (an image file has nothing past it).
                    let pictures = Pictures::of(image.as_ref());
                    let still = pictures == Pictures::Still;
                    if k == 0 {
                        let from = if still { 0 } else { source_frame };
                        self.start_piece(&input_of(&path, image.as_ref()), origin, from, pictures)?;
                    }
                    let k = if still { 0 } else { k };
                    if picture {
                        self.rotation = self.source.as_ref().map_or(0, |s| s.reader.rotation());
                        self.last_decoder = self.source.as_ref().and_then(|s| s.decoder);
                        self.source_frame(k).map(|f| Some(Some(f)))
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
                    self.gpu.as_ref().map(|g| &g.side),
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
        let gpu = self.gpu.as_ref().map(|g| g.side.clone());
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

    /// Moves the current source piece's file on to its output frame `k`
    /// without making the picture.
    fn pass_source_frame(&mut self, k: i64) -> Result<()> {
        let fps = self.fps;
        let src = self.source.as_mut().expect("open");
        let target = target(fps, src.origin, src.source_frame + k);
        if src.advance(target)? {
            src.normalized = None;
        }
        Ok(())
    }

    /// Output frame `k` of the current source piece.
    fn source_frame(&mut self, k: i64) -> Result<frame::Video> {
        let (out_w, out_h, out_fmt) = self.out;
        let fps = self.fps;
        let src = self.source.as_mut().expect("open");
        let target = target(fps, src.origin, src.source_frame + k);
        let changed = src.advance(target)?;
        if let Some(relayout) = self.decoded.as_mut() {
            if changed || src.normalized.is_none() {
                let (frame, _) = src.current.as_ref().expect("have a frame");
                // Frames the GPU decoded come down to memory at their size,
                // unless a display takes NVDEC's or VA-API's from the GPU as
                // they are.
                src.normalized = Some(if crate::hw::on_gpu(frame) {
                    let keep = self.keep_on_gpu && displayable(crate::hw::sw_format(frame));
                    match frame.format() {
                        format::Pixel::CUDA if keep => frame_ref(frame)?,
                        format::Pixel::VAAPI if keep => {
                            crate::hw::Keeper::copy(&mut self.keeper, frame)?
                        }
                        _ => relayout.run(&crate::hw::download(frame)?)?,
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
            src.normalized = Some(self.normalizer.as_mut().expect("built").run(frame)?);
        }
        let picture = frame_ref(src.normalized.as_ref().expect("normalized"))?;
        let mut out = match &mut self.gpu {
            Some(g) if src.on_gpu => {
                g.made += 1;
                picture
            }
            Some(g) => g.frames.upload(&picture)?,
            None => picture,
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
    use super::{fill_black, fit};
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

    #[test]
    fn fitting_keeps_display_aspect() {
        assert_eq!(fit(1920, 1080, (1, 1), 1920, 1080), (1920, 1080));
        assert_eq!(fit(1280, 720, (1, 1), 1920, 1080), (1920, 1080));
        assert_eq!(
            fit(1440, 1080, (1, 1), 1920, 1080),
            (1440, 1080),
            "4:3 pillarboxed"
        );
        assert_eq!(
            fit(1440, 1080, (4, 3), 1920, 1080),
            (1920, 1080),
            "anamorphic 16:9"
        );
        assert_eq!(
            fit(1920, 1080, (1, 1), 1080, 1920),
            (1080, 608),
            "landscape into vertical"
        );
        assert_eq!(fit(720, 576, (16, 15), 1920, 1080), (1440, 1080), "PAL 4:3");
    }
}
