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
use crate::route::{Dest, Facts, Place, Route, Went, displayable};

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
    /// Without `zscale`: its math on the CPU ([`crate::color::split`]).
    cpu: Option<CpuStage>,
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

/// `zscale`'s part of a [`Normalizer`] without it: the colors converted on
/// the CPU, written as planar YUV there (`write`) or not, and the graph from
/// there into the output's format (`back`).
struct CpuStage {
    colors: crate::color::CpuColors,
    write: Option<format::Pixel>,
    target: Option<Hdr>,
    back: Option<filter::Graph>,
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

/// A frame in GPU memory brought down for the CPU to normalize into an
/// `out_w`×`out_h` picture: scaled on the GPU first to the size it is shown
/// at there (in the frame's own orientation and layout, its pixels square)
/// when that is smaller, so a fraction comes down. The CPU does the rest as
/// for any frame: turning, colors, HDR.
pub(crate) fn down(
    shrinker: &mut Option<crate::hw::Shrinker>,
    src: &frame::Video,
    rotation: u16,
    out: (u32, u32),
) -> Result<frame::Video> {
    let (w, h) = (src.width(), src.height());
    let size = shown_size(src, rotation, out);
    let shrinks = size.0 < w && size.1 < h;
    if !shrinks {
        return crate::hw::download(src);
    }
    let mut small = crate::hw::Shrinker::down(shrinker, src, size)?;
    // Unless it came down whole.
    if (small.width(), small.height()) == size {
        // SAFETY: a plain field of its own frame.
        unsafe {
            (*small.as_mut_ptr()).sample_aspect_ratio = ffmpeg::ffi::AVRational { num: 1, den: 1 }
        };
    }
    Ok(small)
}

/// The size `src` is shown at in an `out` picture, as the normalizer fits
/// it upright: as shown, a quarter turn swapping the sides and the pixel
/// aspect.
fn fitted(src: &frame::Video, rotation: u16, (out_w, out_h): (u32, u32)) -> (u32, u32) {
    let (w, h) = (src.width(), src.height());
    let sar = src.aspect_ratio();
    let sar = (sar.numerator(), sar.denominator());
    if rotation % 180 == 90 {
        fit(h, w, (sar.1, sar.0), out_w, out_h)
    } else {
        fit(w, h, sar, out_w, out_h)
    }
}

/// [`fitted`], in the frame's own orientation.
fn shown_size(src: &frame::Video, rotation: u16, out: (u32, u32)) -> (u32, u32) {
    let (w, h) = fitted(src, rotation, out);
    if rotation % 180 == 90 { (h, w) } else { (w, h) }
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
        let zscale = crate::parts::has_filter("zscale");
        Self::with(src, rotation, (out_w, out_h), out_fmt, target, zscale)
    }

    /// [`Normalizer::new`], converting colors with `zscale` or, without
    /// it, on the CPU.
    fn with(
        src: &frame::Video,
        rotation: u16,
        (out_w, out_h): (u32, u32),
        out_fmt: format::Pixel,
        target: Option<Hdr>,
        zscale: bool,
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
        // Without zimg the CPU does `zscale`'s part, between two graphs.
        let (conv, cpu) = if crate::color::needs_zscale(color, target) && !zscale {
            let split = crate::color::split(color, out_fmt, target);
            (split.before, Some((split.cpu, split.write, split.back)))
        } else {
            (crate::color::conversion(color, out_fmt, target), None)
        };
        let last = match cpu {
            Some(_) => String::new(),
            None => format!(",format={}", fmt_name(out_fmt)),
        };
        let spec = format!(
            "{}scale={sw}:{sh}:flags={}{},pad={out_w}:{out_h}:{}:{}:color=black,setsar=1{}{last}",
            upright(rotation),
            conv.flags,
            conv.scale,
            (out_w - sw) / 2,
            (out_h - sh) / 2,
            conv.after,
        );
        crate::parts::parse_filters(&mut graph, &spec)?;
        graph
            .validate()
            .ctx(|| format!("configuring filter {spec}"))?;
        let cpu = match cpu {
            Some((colors, write, back)) => {
                let from = write.unwrap_or(format::Pixel::GBRPF32LE);
                let args = format!(
                    "video_size={out_w}x{out_h}:pix_fmt={}:time_base=1/1000000:pixel_aspect=1/1",
                    ffmpeg::ffi::AVPixelFormat::from(from) as i32
                );
                let back = match back {
                    Some(spec) => Some(simple_graph(&args, &spec)?),
                    None => None,
                };
                Some(CpuStage {
                    colors,
                    write,
                    target,
                    back,
                })
            }
            None => None,
        };
        Ok(Self {
            graph,
            cpu,
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
        let Some(cpu) = self.cpu.as_mut() else {
            return Ok(out);
        };
        // SAFETY: the graph's frame, made the caller's own to write.
        if unsafe { ffmpeg::ffi::av_frame_make_writable(out.as_mut_ptr()) } < 0 {
            return Err(Error::Invalid("no memory for a frame".into()));
        }
        cpu.colors.run(&mut out);
        if let Some(pixel) = cpu.write {
            let mut yuv = frame::Video::new(pixel, out.width(), out.height());
            crate::color::write_yuv(&out, &mut yuv, cpu.target);
            yuv.set_pts(out.pts());
            out = yuv;
        }
        let Some(back) = cpu.back.as_mut() else {
            return Ok(out);
        };
        back.get("in")
            .expect("source")
            .source()
            .add(&out)
            .ctx(|| "filtering frame".into())?;
        let mut written = frame::Video::empty();
        back.get("out")
            .expect("sink")
            .sink()
            .frame(&mut written)
            .ctx(|| "filtering frame".into())?;
        Ok(written)
    }
}

/// A graph of `spec` between a source of `args` and a sink.
fn simple_graph(args: &str, spec: &str) -> Result<filter::Graph> {
    let find =
        |name: &str| filter::find(name).ok_or_else(|| Error::Invalid(format!("no {name} filter")));
    let mut graph = filter::Graph::new();
    graph
        .add(&find("buffer")?, "in", args)
        .ctx(|| "creating video source".into())?;
    graph
        .add(&find("buffersink")?, "out", "")
        .ctx(|| "creating video sink".into())?;
    crate::parts::parse_filters(&mut graph, spec)?;
    graph
        .validate()
        .ctx(|| format!("configuring filter {spec}"))?;
    Ok(graph)
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
    /// `current`, made into its output picture; `made_on_gpu`: made there
    /// for the encoder there.
    normalized: Option<frame::Video>,
    made_on_gpu: bool,
    /// A still keeps its one picture and is never sought; a sequence knows
    /// how many pictures it has.
    pictures: Pictures,
    /// Its frames' way into the output ([`Maker`]).
    fitting: Fitting,
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
        let fitting = match gpu {
            Some(g) => Fitting::for_nvenc(&reader, g.out, g.format, g.target, true),
            None => Fitting::default(),
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
            made_on_gpu: false,
            pictures,
            fitting,
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
        if let Some(scaler) = source.fitting.scaler.as_mut() {
            let (frame, _) = source.current.as_ref().expect("have a frame");
            source.normalized = Some(cuda_picture(scaler, frame)?);
            source.made_on_gpu = true;
        }
        Ok(source)
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
            made_on_gpu: false,
            pictures,
            fitting: Fitting::default(),
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
    piece: usize,
    /// Frame index within the current piece.
    k: i64,
    source: Option<Source>,
    /// The next file, opened ahead; named by where it opens.
    ahead: Ahead<Source, (Input, u64, i64)>,
    black: Option<frame::Video>,
    /// The GPU frames decode on ([`decode_on`](Self::decode_on)).
    decoder: Option<crate::hw::GpuDecoder>,
    /// Makes each output picture from the source frame it shows.
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
    /// Pictures of a piece walking its source backward, made ahead.
    reversed: Vec<Reversed>,
}

/// What a caller makes of the frames ([`VideoFrames::output`]).
pub(crate) enum Output {
    /// Fitted into the output, in memory: the default.
    Fitted,
    /// Each frame as decoded: its own size and colors, upright only after
    /// [`rotation`](VideoFrames::rotation), in a layout a display takes (see
    /// `Relayout`). Black is 8-bit 4:2:0. With `keep_on_gpu`, the GPU's
    /// frames stay in its memory in NV12 or P010, for a display that shows
    /// them from there.
    Decoded { keep_on_gpu: bool },
    /// Fitted into the output for an encoder that takes GPU frames.
    Encoder(EncoderFrames),
}

/// An encoder's frames on a GPU ([`Output::Encoder`]).
pub(crate) struct EncoderFrames {
    /// Where it takes the decoder's frames as they are, when it is on the
    /// GPU that decodes: those fitted there go to it without a copy.
    pub(crate) place: Option<Place>,
    /// Where pictures made on the CPU, and black, go up to it.
    pub(crate) up: Up,
    /// Direct3D 11: its own frames, which the video processor fits frames
    /// into (AMF takes those alone).
    pub(crate) into: Option<crate::hw::Frames>,
}

/// Where pictures made on the CPU go up to an encoder.
pub(crate) enum Up {
    /// NVENC's CUDA frames, on its device, whose NVDEC decodes its pieces
    /// on streams of their own.
    Cuda {
        device: crate::cuda::CudaDevice,
        frames: crate::cuda::CudaFrames,
    },
    /// VA-API's, Direct3D 11's or Vulkan's frames.
    Frames(super::encode::Upload),
}

/// Makes output pictures from the source frames they show, along each
/// source's route (`route::plan`): fitted on the GPU that decoded them and
/// handed on there, shrunk there and finished on the CPU, or made on the
/// CPU. The one engine for every job that fits decoded frames into an
/// output.
pub(crate) struct Maker {
    /// The output's size and layout.
    pub(crate) out: (u32, u32, format::Pixel),
    /// The output's dynamic range (`None`: SDR BT.709).
    pub(crate) target: Option<Hdr>,
    /// What the pictures are made into.
    pub(crate) output: Output,
    /// Fits frames into the output on the CPU.
    normalizer: Option<Normalizer>,
    /// Shrinks the frames a GPU decoded before they come down.
    shrinker: Option<crate::hw::Shrinker>,
    /// Brings frames as decoded into a layout a display takes.
    relayout: Relayout,
    /// Keeps the GPU's frames there ([`crate::hw::Keeper`]); `unkept`: that
    /// failed, and they come down from then on.
    keeper: Option<crate::hw::Keeper>,
    unkept: bool,
    /// Fits VA-API's and Direct3D 11's frames into the output on their GPU.
    fitter: Option<crate::hw::Fitter>,
}

/// A source's way into the output ([`Maker`]): its route, planned at its
/// first frame, and NVDEC's scaler for NVENC.
#[derive(Default)]
pub(crate) struct Fitting {
    route: Option<Route>,
    /// Fitting its frames on the GPU failed: the route was planned again.
    fit_failed: bool,
    scaler: Option<crate::cuda::CudaPictures>,
    /// How its last frame went.
    pub(crate) went: Option<Went>,
}

impl Fitting {
    /// For NVENC: the frames `reader` decodes on NVDEC, scaled on the GPU
    /// to `out` in `format` and `target`'s dynamic range, where the scaler
    /// builds (libplacebo needs Vulkan for turned pictures, other matrices
    /// than BT.709, and HDR). The CPU makes those it won't take.
    /// `at_once`: each picture is wanted as its frame goes in, not with the
    /// next one ([`Maker::make`]).
    pub(crate) fn for_nvenc(
        reader: &VideoReader,
        out: (u32, u32),
        format: format::Pixel,
        target: Option<Hdr>,
        at_once: bool,
    ) -> Self {
        let Some(device) = reader.cuda() else {
            return Self::default();
        };
        let scaler = crate::cuda::CudaPictures::new(device, reader, out, format, target, at_once);
        Self::nvdec(scaler.ok())
    }

    /// For a Vulkan encoder on NVDEC's GPU: the frames `reader` decodes
    /// there, made into `out` pictures of `format` by libplacebo and left
    /// in Vulkan. The CPU makes those it won't take.
    pub(crate) fn for_vulkan(
        reader: &VideoReader,
        out: (u32, u32),
        format: format::Pixel,
        at_once: bool,
    ) -> Self {
        let Some(device) = reader.cuda() else {
            return Self::default();
        };
        let made = crate::cuda::CudaPictures::for_vulkan(device, reader, out, format, at_once);
        if let Err(e) = &made {
            crate::debug!(
                "decoding",
                "{}: libplacebo won't make the pictures, the CPU does: {e}",
                reader.path().display()
            );
        }
        Self::nvdec(made.ok())
    }

    /// NVDEC's frames fitted on the GPU by `scaler`, or the CPU's to make
    /// without one.
    fn nvdec(scaler: Option<crate::cuda::CudaPictures>) -> Self {
        Self {
            route: scaler.is_some().then_some(Route::Gpu),
            fit_failed: scaler.is_none(),
            scaler,
            went: None,
        }
    }
}

/// NVDEC's `frame`, scaled for NVENC by `scaler`.
fn cuda_picture(
    scaler: &mut crate::cuda::CudaPictures,
    frame: &frame::Video,
) -> Result<frame::Video> {
    scaler.push(Some(frame))?;
    scaler
        .pull()?
        .ok_or_else(|| Error::Invalid("the GPU scaler gave no picture".into()))
}

impl Maker {
    pub(crate) fn new(out: (u32, u32, format::Pixel), target: Option<Hdr>) -> Self {
        Self {
            out,
            target,
            output: Output::Fitted,
            normalizer: None,
            shrinker: None,
            relayout: Relayout::new(),
            keeper: None,
            unkept: false,
            fitter: None,
        }
    }

    /// Where the pictures go, for the planner.
    fn dest(&self) -> Dest {
        match &self.output {
            Output::Fitted => Dest::Memory,
            Output::Decoded { keep_on_gpu } => Dest::Display {
                keep: *keep_on_gpu && !self.unkept,
            },
            Output::Encoder(e) => match e.place {
                Some(place) => Dest::Encoder { place },
                None => Dest::Memory,
            },
        }
    }

    /// NVENC's CUDA device, whose NVDEC decodes the pieces.
    pub(crate) fn cuda_device(&self) -> Option<&crate::cuda::CudaDevice> {
        match &self.output {
            Output::Encoder(EncoderFrames {
                up: Up::Cuda { device, .. },
                ..
            }) => Some(device),
            _ => None,
        }
    }

    /// The encoder's own Direct3D 11 frames, to fit frames into.
    fn into(&self) -> Option<&crate::hw::Frames> {
        match &self.output {
            Output::Encoder(e) => e.into.as_ref(),
            _ => None,
        }
    }

    /// What the planner needs to know of `frame`'s source, shown turned by
    /// `rotation`.
    fn facts(&self, frame: &frame::Video, rotation: u16, fit_failed: bool) -> Facts {
        let place = Place::of(frame);
        let layout = match place {
            Place::Memory => frame.format(),
            _ => crate::hw::sw_format(frame),
        };
        let color = crate::color::Source::of(frame);
        let out = (self.out.0, self.out.1);
        let (w, h) = shown_size(frame, rotation, out);
        Facts {
            place,
            layout,
            rotation,
            rgb: color.rgb(),
            colors_kept: crate::color::kept(color, self.target),
            shrinks: w < frame.width() && h < frame.height(),
            pads: fitted(frame, rotation, out) != out,
            out_layout: self.out.2,
            into: self.into().is_some(),
            fit_failed,
        }
    }

    /// The picture of `frame`, its source's frame shown turned by
    /// `rotation`, along the source's route; and whether the GPU made it,
    /// for the encoder there. `None`: NVIDIA's scaler, made to save its GPU
    /// work, gives it with the next frame ([`rest`](Self::rest) gives the
    /// last); such a picture keeps its source frame's timestamp. `path`
    /// names the file in what `VIDCUT_DEBUG` says.
    pub(crate) fn make(
        &mut self,
        frame: &frame::Video,
        rotation: u16,
        fitting: &mut Fitting,
        path: &Path,
    ) -> Result<Option<(frame::Video, bool)>> {
        // NVDEC's frames fit on the GPU only through the scaler their
        // source made at opening.
        if fitting.route.is_none() && Place::of(frame) == Place::Cuda && fitting.scaler.is_none() {
            fitting.fit_failed = true;
        }
        loop {
            let route = match fitting.route {
                Some(r) => r,
                None => {
                    let facts = self.facts(frame, rotation, fitting.fit_failed);
                    let r = crate::route::plan(&facts, self.dest());
                    crate::debug!("decoding", "{}: {r:?}, {facts:?}", path.display());
                    *fitting.route.insert(r)
                }
            };
            match route {
                Route::Kept => match crate::hw::Keeper::keep(&mut self.keeper, frame) {
                    Ok(kept) => {
                        fitting.went = Some(Went::Stayed);
                        return Ok(Some((kept, false)));
                    }
                    // Down to memory, as before they were kept: a copy that
                    // failed or took two seconds would again.
                    Err(e) => {
                        eprintln!("vidcut: GPU pictures come down to be shown: {e}");
                        self.unkept = true;
                        self.keeper = None;
                        fitting.route = None;
                    }
                },
                Route::Gpu => {
                    if let Some(scaler) = fitting.scaler.as_mut() {
                        scaler.push(Some(frame))?;
                        let picture = scaler.pull()?;
                        fitting.went = Some(Went::Stayed);
                        return Ok(picture.map(|p| (p, true)));
                    }
                    // A GPU that won't fit them (a driver's limits) leaves
                    // the file's frames to the CPU.
                    match self.fit(frame, rotation) {
                        Ok(picture) => {
                            fitting.went = Some(Went::Stayed);
                            return Ok(Some((picture, true)));
                        }
                        Err(e) => {
                            crate::debug!(
                                "decoding",
                                "{}: the GPU won't fit the frames for the encoder, the CPU makes them: {e}",
                                path.display()
                            );
                            fitting.fit_failed = true;
                            fitting.route = None;
                        }
                    }
                }
                Route::Shrunk | Route::Whole | Route::Cpu => {
                    let (picture, went) = self.on_cpu(frame, rotation)?;
                    fitting.went = Some(went);
                    return Ok(Some((picture, false)));
                }
            }
        }
    }

    /// The pictures a source's scaler still holds once its last frame went
    /// in ([`make`](Self::make)).
    pub(crate) fn rest(&mut self, fitting: &mut Fitting) -> Result<Vec<frame::Video>> {
        let mut rest = Vec::new();
        if let Some(scaler) = fitting.scaler.as_mut() {
            scaler.push(None)?;
            while let Some(p) = scaler.pull()? {
                rest.push(p);
            }
        }
        Ok(rest)
    }

    /// `frame` fitted into the output on VA-API's or Direct3D 11's GPU, for
    /// the encoder there.
    fn fit(&mut self, frame: &frame::Video, rotation: u16) -> Result<frame::Video> {
        let (out_w, out_h, out_fmt) = self.out;
        let fitted = fitted(frame, rotation, (out_w, out_h));
        let into = match &self.output {
            Output::Encoder(e) => e.into.as_ref(),
            _ => None,
        };
        crate::hw::Fitter::fit(
            &mut self.fitter,
            frame,
            rotation,
            fitted,
            (out_w, out_h),
            out_fmt,
            into,
        )
    }

    /// `frame` made on the CPU: copied down first if a GPU decoded it,
    /// shrunk there to the size it is shown at; as decoded for a display.
    /// And how it went.
    fn on_cpu(&mut self, frame: &frame::Video, rotation: u16) -> Result<(frame::Video, Went)> {
        let decoded = matches!(self.output, Output::Decoded { .. });
        let came_down;
        let (frame, went) = if crate::hw::on_gpu(frame) {
            // Frames as decoded come down at their size.
            came_down = if decoded {
                crate::hw::download(frame)?
            } else {
                down(
                    &mut self.shrinker,
                    frame,
                    rotation,
                    (self.out.0, self.out.1),
                )?
            };
            let size = (came_down.width(), came_down.height());
            let whole = size == (frame.width(), frame.height());
            (&came_down, Went::Down { size, whole })
        } else {
            (frame, Went::Cpu)
        };
        if decoded {
            return Ok((self.relayout.run(frame)?, went));
        }
        let (out_w, out_h, out_fmt) = self.out;
        if self
            .normalizer
            .as_ref()
            .is_none_or(|n| !n.matches(frame, rotation))
        {
            self.normalizer = Some(Normalizer::new(
                frame,
                rotation,
                out_w,
                out_h,
                out_fmt,
                self.target,
            )?);
        }
        Ok((self.normalizer.as_mut().expect("built").run(frame)?, went))
    }

    /// `picture`, made on the CPU, as the encoder takes it: up into its GPU
    /// frames.
    pub(crate) fn up(&self, picture: frame::Video) -> Result<frame::Video> {
        match &self.output {
            Output::Encoder(EncoderFrames {
                up: Up::Cuda { frames, .. },
                ..
            }) => frames.upload(&picture),
            Output::Encoder(EncoderFrames {
                up: Up::Frames(upload),
                ..
            }) => upload.upload(&picture),
            _ => Ok(picture),
        }
    }

    /// Whether the encoder's frames are on the GPU that decodes, whose
    /// pools are small.
    fn on_encoder_gpu(&self) -> bool {
        matches!(&self.output, Output::Encoder(e) if e.place.is_some())
    }
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
        // told nothing, swscale would convert to BT.601. RGB is converted,
        // with the flags that round its colors right.
        let spec = if rgb {
            let flags = crate::color::CONVERT;
            format!("scale=out_color_matrix=bt709:out_range=limited:flags={flags},format={target}")
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
        crate::parts::parse_filters(&mut graph, &spec)?;
        graph
            .validate()
            .ctx(|| format!("configuring filter {spec}"))?;
        Ok(graph)
    }
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
            source: None,
            ahead: Ahead::new(),
            black: None,
            decoder: None,
            maker: Maker::new((width, height, pixel), target),
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
            self.maker.out.2 = format::Pixel::YUV420P;
        }
        self.maker.output = output;
        self
    }

    /// Frames decode on `gpu` where it decodes the stream
    /// ([`VideoReader::open_gpu_or_cpu`]): the same frames, with the same
    /// timestamps and colors. Frames as decoded ([`Output::Decoded`]) are
    /// copied down to memory at their own size; frames fitted into the
    /// output come down shrunk on the GPU to the size they are shown at,
    /// where it is smaller ([`down`]). Other streams, stills and image
    /// sequences decode on the CPU, and pieces NVDEC takes for NVENC
    /// ([`Up::Cuda`]) stay with it.
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

    /// What a source decoding for NVENC needs ([`Up::Cuda`]).
    fn gpu_side(&self) -> Option<GpuSide> {
        let device = self.maker.cuda_device()?;
        let (w, h, format) = self.maker.out;
        Some(GpuSide {
            device: device.clone(),
            out: (w, h),
            format,
            target: self.maker.target,
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
                    let mut out = self.maker.up(black)?;
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
            let (w, h, pix) = self.maker.out;
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
        let (w, h) = match (&self.maker.output, current) {
            (Output::Decoded { .. }, Some((f, _))) => (f.width(), f.height()),
            _ => (self.maker.out.0, self.maker.out.1),
        };
        let bytes = (w as usize * h as usize * 3).max(1);
        let most = if self.maker.on_encoder_gpu() {
            REVERSED_ON_GPU
        } else {
            REVERSED_MOST
        };
        ((REVERSED_BYTES / bytes) as i64).clamp(1, most)
    }

    /// The output frame of the open file's source frame `pos`.
    fn frame_at(&mut self, pos: i64) -> Result<frame::Video> {
        let fps = self.fps;
        let src = self.source.as_mut().expect("open");
        let target = target(fps, src.origin, pos);
        let changed = src.advance(target)?;
        if changed || src.normalized.is_none() {
            let (frame, _) = src.current.as_ref().expect("have a frame");
            let rotation = src.reader.rotation();
            let made = self
                .maker
                .make(frame, rotation, &mut src.fitting, src.reader.path())?;
            // Its scaler gives each picture at once.
            let (picture, on_gpu) =
                made.ok_or_else(|| Error::Invalid("the GPU scaler gave no picture".into()))?;
            src.normalized = Some(picture);
            src.made_on_gpu = on_gpu;
        }
        let picture = frame_ref(src.normalized.as_ref().expect("normalized"))?;
        self.gpu_made += i64::from(src.made_on_gpu);
        let mut out = if src.made_on_gpu {
            picture
        } else {
            self.maker.up(picture)?
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
