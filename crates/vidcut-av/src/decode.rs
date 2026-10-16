//! Video decoding with frame-exact seeking.
//!
//! Seek semantics match FFmpeg's accurate seek: after
//! [`VideoReader::seek`] to `target`, the next frame returned is the first frame
//! whose presentation timestamp is `>= target`. Callers put the half-frame bias
//! into `target` (see the time map), so that rounding in timestamps never selects
//! the neighboring frame.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, codec, filter, format, frame, media, software::scaling};

/// An audio frame that starts later than the frame before it ended, by more
/// than this (s), follows a gap: a damaged packet, or one the file lacks.
/// Smaller differences are timestamp rounding (Matroska keeps ms).
pub(crate) const AUDIO_GAP: f64 = 0.002;

/// Longest gap (s) kept when the container's end is unknown.
///
/// A longer jump is a broken timestamp and plays in order, as a frame past
/// a known end does. Filling it would silence the rest of the piece, a
/// second at a time, for as long as the timestamp says.
pub(crate) const AUDIO_GAP_BOUND: f64 = 1.0;

/// Damaged frames a decoder may report in a row before reading gives up.
/// It reads on after each one, so only a decoder that reports nothing else
/// reaches this.
pub(crate) const DAMAGED_IN_A_ROW: u32 = 100;

/// End of a file in seconds, on the clock packet timestamps use.
///
/// `start` and `duration` are the container's (`None` when unset).
/// `duration` is a length. A known start places it at `start + duration`.
/// With no start, `origin` does (the stream's start): counting the length
/// from zero puts an offset clock past the end, a real gap is dropped, and
/// later audio shifts early. No positive duration, or no start at all, means
/// the end is unknown.
pub(crate) fn file_end(
    start: Option<f64>,
    duration: Option<f64>,
    origin: Option<f64>,
) -> Option<f64> {
    let duration = duration.filter(|d| *d > 0.0)?;
    Some(start.or(origin)? + duration)
}

/// Silence, in seconds, before a frame at `t` when the previous frame ended
/// at `next`. `end` is [`file_end`], if the container has one.
///
/// A frame past a known end plays in order. So does a jump longer than
/// [`AUDIO_GAP_BOUND`] when the end is unknown: one far timestamp must not
/// become silence for the rest of the piece.
pub(crate) fn audio_gap(next: f64, t: f64, end: Option<f64>) -> f64 {
    let jump = t - next;
    if jump <= AUDIO_GAP {
        return 0.0;
    }
    match end {
        Some(end) if t <= end => jump,
        Some(_) => 0.0,
        None if jump <= AUDIO_GAP_BOUND => jump,
        None => 0.0,
    }
}

/// Stream start in seconds, if the demuxer recorded one. `tick` is the
/// stream time base in seconds.
pub(crate) fn stream_origin(start_pts: i64, tick: f64) -> Option<f64> {
    (start_pts != ffmpeg::ffi::AV_NOPTS_VALUE).then_some(start_pts as f64 * tick)
}

fn opt_av_time(ts: i64) -> Option<f64> {
    (ts != ffmpeg::ffi::AV_NOPTS_VALUE).then_some(ts as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE))
}

/// [`file_end`] for an open input.
///
/// `stream_start` is that stream's `start_time` (stream time base) and
/// `tick` its time base in seconds. When the container has a duration but
/// no start, the length is placed on the stream's clock.
pub(crate) fn container_end(
    input: &format::context::Input,
    stream_start: i64,
    tick: f64,
) -> Option<f64> {
    let (start, duration) = unsafe {
        let ctx = &*input.as_ptr();
        (opt_av_time(ctx.start_time), opt_av_time(ctx.duration))
    };
    file_end(start, duration, stream_origin(stream_start, tick))
}

/// Sends `packet` to `decoder`. A damaged packet, one the decoder can't
/// read, is skipped as players skip it: the picture before it stays, and
/// audio keeps to its timestamps across the gap.
pub(crate) fn send_packet(
    decoder: &mut ffmpeg::decoder::Opened,
    packet: &ffmpeg::Packet,
) -> Result<()> {
    match decoder.send_packet(packet) {
        Ok(()) | Err(ffmpeg::Error::InvalidData) => Ok(()),
        Err(e) => Err(Error::Ffmpeg {
            context: "sending packet to decoder".into(),
            source: e,
        }),
    }
}

/// Decodes stream `index` of `input` to its end, handing `each` every
/// frame (decoded into `frame`). Damaged packets and frames are skipped,
/// as [`send_packet`] skips them.
pub(crate) fn decode_stream<F>(
    input: &mut format::context::Input,
    index: usize,
    decoder: &mut ffmpeg::decoder::Opened,
    frame: &mut F,
    mut each: impl FnMut(&F) -> Result<()>,
) -> Result<()>
where
    F: std::ops::DerefMut<Target = frame::Frame>,
{
    let mut damaged = 0;
    let mut drain = |decoder: &mut ffmpeg::decoder::Opened, frame: &mut F| loop {
        // The same flag the export frame loop loads, so a client that left
        // stops a measurement between frames.
        crate::cancel::stop()?;
        crate::first::wait()?;
        match decoder.receive_frame(frame) {
            Ok(()) => {
                damaged = 0;
                each(frame)?
            }
            Err(ffmpeg::Error::InvalidData) if damaged < DAMAGED_IN_A_ROW => damaged += 1,
            Err(
                ffmpeg::Error::Eof
                | ffmpeg::Error::Other {
                    errno: ffmpeg::util::error::EAGAIN,
                },
            ) => return Ok(()),
            Err(e) => {
                return Err(Error::Ffmpeg {
                    context: "decoding".into(),
                    source: e,
                });
            }
        }
    };
    for (s, packet) in input.packets() {
        crate::cancel::stop()?;
        if s.index() == index {
            send_packet(decoder, &packet)?;
            drain(decoder, frame)?;
        }
    }
    decoder.send_eof().ctx(|| "flushing decoder".into())?;
    drain(decoder, frame)
}

/// Where pictures come from: a media file, or the numbered files of an
/// image sequence, which FFmpeg's image2 demuxer reads as one stream:
/// picture `k` at `k / rate`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Input {
    File(PathBuf),
    /// `pattern` names the files (image2's printf form, `frame_%06d.png`)
    /// from the number `first` on; `rate` is (numerator, denominator).
    Sequence {
        pattern: PathBuf,
        first: u64,
        rate: (i32, i32),
    },
}

impl Input {
    /// The file, or the sequence's pattern.
    pub fn path(&self) -> &Path {
        match self {
            Input::File(p) => p,
            Input::Sequence { pattern, .. } => pattern,
        }
    }

    /// Opens it for demuxing.
    pub fn open(&self) -> Result<format::context::Input> {
        crate::init();
        match self {
            Input::File(p) => open_file(p),
            Input::Sequence {
                pattern,
                first,
                rate,
            } => open_sequence(pattern, *first, *rate),
        }
    }
}

/// Opens the media file at `path` for demuxing. A path from another kind
/// of system names no file here (`vidcut_core::store::from_another_system`):
/// on Windows, `/Users/x/a.mp4` would open another file on this drive.
pub(crate) fn open_file(path: &Path) -> Result<format::context::Input> {
    if vidcut_core::store::from_another_system(path) {
        return Err(Error::Invalid(format!(
            "{} is a path on another kind of system: relink the media",
            path.display()
        )));
    }
    format::input(path).ctx(|| format!("opening {}", path.display()))
}

fn open_sequence(pattern: &Path, first: u64, rate: (i32, i32)) -> Result<format::context::Input> {
    let what = || format!("opening {}", pattern.display());
    let mut options = ffmpeg::Dictionary::new();
    options.set("pattern_type", "sequence");
    options.set("start_number", &first.to_string());
    // The default range is 5: a missing first file is skipped when any of
    // the next four numbers exists, and picture 0 is that later file.
    options.set("start_number_range", "1");
    options.set("framerate", &format!("{}/{}", rate.0, rate.1));
    let path = std::ffi::CString::new(pattern.as_os_str().as_encoded_bytes())
        .map_err(|_| Error::Invalid(format!("{}: a NUL in the name", pattern.display())))?;
    unsafe {
        let image2 = ffmpeg::ffi::av_find_input_format(c"image2".as_ptr());
        let mut ctx = std::ptr::null_mut();
        let mut opts = options.disown();
        let ret = ffmpeg::ffi::avformat_open_input(&mut ctx, path.as_ptr(), image2, &mut opts);
        drop(ffmpeg::Dictionary::own(opts));
        if ret < 0 {
            return Err(Error::Ffmpeg {
                context: what(),
                source: ffmpeg::Error::from(ret),
            });
        }
        let ret = ffmpeg::ffi::avformat_find_stream_info(ctx, std::ptr::null_mut());
        if ret < 0 {
            ffmpeg::ffi::avformat_close_input(&mut ctx);
            return Err(Error::Ffmpeg {
                context: what(),
                source: ffmpeg::Error::from(ret),
            });
        }
        Ok(format::context::Input::wrap(ctx))
    }
}

/// Video frames decoded by every reader in the process so far.
static DECODED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many video frames every reader in the process has decoded so far: a
/// measure of decoding work, for tests and diagnostics.
pub fn frames_decoded() -> u64 {
    DECODED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Decodes the primary video stream of a file in presentation order.
pub struct VideoReader {
    source: Input,
    input: format::context::Input,
    stream: usize,
    time_base: ffmpeg::Rational,
    start_pts: Option<i64>,
    decoder: ffmpeg::decoder::Video,
    /// EOF has been sent to the decoder.
    draining: bool,
    /// Frames with pts below this are dropped (set by `seek`).
    min_pts: Option<i64>,
    /// One frame of look-ahead, filled while verifying a seek.
    pending: Option<frame::Video>,
    /// A packet read by a seek and not yet sent to the decoder.
    packet: Option<ffmpeg::Packet>,
    /// Pts of the first decodable frame (`o` in the time map).
    first_pts: Option<i64>,
    /// Clockwise rotation that shows the pictures upright.
    rotation: u16,
    /// What the first decodable frame says about fields and color.
    first_props: Option<FrameProps>,
    /// For interlaced streams: every decoded frame goes through it, before
    /// anyone picks frames (on the GPU for frames NVDEC and Vulkan decode,
    /// where its filter gives `bwdif`'s).
    deinterlacer: Option<Deinterlacer>,
    /// The stream's frames are interlaced.
    interlaced: bool,
    /// The GPU decoding, when one does, and the kind of its decoders that
    /// decodes the stream: its own, or Vulkan's.
    gpu: Option<crate::hw::GpuDecoder>,
    kind: Option<crate::hw::Kind>,
    /// What the file holds and what decodes it ([`decoding`](Self::decoding)).
    decoding: Arc<Decoding>,
}

/// What a file holds and what decodes it, for people.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Decoding {
    /// FFmpeg's names of the codec and of its profile: `h264`, `High`.
    pub codec: String,
    pub profile: Option<String>,
    /// The stream's frame rate, when it gives one.
    pub fps: Option<f64>,
    pub interlaced: bool,
    /// The clockwise turn that shows the pictures upright: 0, 90, 180 or 270.
    pub rotation: u16,
    /// FFmpeg's decoder: `h264`, `hevc`.
    pub decoder: String,
    /// The GPU decoder it runs on: `NVDEC on NVIDIA GeForce RTX 4070`.
    /// `None` is the CPU.
    pub gpu: Option<String>,
    /// Why it runs on the CPU though a GPU was asked.
    pub why: Option<String>,
}

impl std::fmt::Display for Decoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.codec)?;
        if let Some(p) = &self.profile {
            write!(f, " {p}")?;
        }
        if let Some(fps) = self.fps {
            let fps = format!("{fps:.3}");
            write!(
                f,
                " at {} fps",
                fps.trim_end_matches('0').trim_end_matches('.')
            )?;
        }
        if self.interlaced {
            f.write_str(", interlaced")?;
        }
        if self.rotation != 0 {
            write!(f, ", shown turned {}°", self.rotation)?;
        }
        let on = self.gpu.as_deref().unwrap_or("the CPU");
        write!(f, "; FFmpeg's {} on {on}", self.decoder)?;
        match &self.why {
            Some(why) => write!(f, " ({why})"),
            None => Ok(()),
        }
    }
}

/// One of FFmpeg's static names; `None` for null.
fn c_name(p: *const std::ffi::c_char) -> Option<String> {
    // SAFETY: FFmpeg's static names, or null.
    (!p.is_null()).then(|| {
        unsafe { std::ffi::CStr::from_ptr(p) }
            .to_string_lossy()
            .into_owned()
    })
}

/// FFmpeg's deinterlacer for frames of `pixel`: `bwdif_cuda` for NVDEC's,
/// `bwdif_vulkan` for Vulkan's, `bwdif` for frames in memory.
pub(crate) fn deinterlacer(pixel: format::Pixel) -> &'static str {
    match pixel {
        format::Pixel::CUDA => "bwdif_cuda",
        format::Pixel::VULKAN => "bwdif_vulkan",
        _ => "bwdif",
    }
}

/// The graph that deinterlaces frames like `f`, timed in `time_base`, with
/// its [`deinterlacer`]: on the GPU the same options as `bwdif`'s, and so
/// the same frames at the same times. Not always the same levels: the GPU's
/// are trusted only where they give `bwdif`'s ([`crate::hw::deinterlace`]).
pub(crate) fn deinterlacing(
    f: &frame::Video,
    time_base: ffmpeg::Rational,
) -> Result<crate::hw::GpuGraph> {
    let sar = f.aspect_ratio();
    let mut graph = crate::hw::GpuGraph::new();
    let find =
        |name: &str| filter::find(name).ok_or_else(|| Error::Invalid(format!("no {name} filter")));
    if crate::hw::on_gpu(f) {
        // SAFETY: reads the frame's own field.
        let frames = unsafe { (*f.as_ptr()).hw_frames_ctx };
        if frames.is_null() {
            return Err(Error::Invalid("GPU frames without a frames context".into()));
        }
        crate::cuda::add_source(
            &mut graph,
            c"in",
            ffmpeg::ffi::AVPixelFormat::from(f.format()),
            (f.width(), f.height()),
            (time_base.into(), sar),
            frames,
            None,
        )?;
    } else {
        let args = format!(
            "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect={}/{}",
            f.width(),
            f.height(),
            ffmpeg::ffi::AVPixelFormat::from(f.format()) as i32,
            time_base.numerator(),
            time_base.denominator(),
            sar.numerator().max(1),
            sar.denominator().max(1)
        );
        graph
            .add(&find("buffer")?, "in", &args)
            .ctx(|| "creating deinterlacer source".into())?;
    }
    graph
        .add(&find("buffersink")?, "out", "")
        .ctx(|| "creating deinterlacer sink".into())?;
    let spec = format!(
        "{}=mode=send_frame:parity=auto:deint=interlaced",
        deinterlacer(f.format())
    );
    graph.parse_filters(&spec)?;
    graph.validate().ctx(|| format!("configuring {spec}"))?;
    Ok(graph)
}

/// `bwdif` (or `bwdif_cuda`, `bwdif_vulkan`) over a stream's frames in
/// order: one progressive frame for each interlaced one, made from both its
/// fields with the frames around it (send_frame); progressive frames pass
/// through. Holds one frame back.
struct Deinterlacer {
    graph: Option<crate::hw::GpuGraph>,
    /// The stream's time base, and the parameters the graph was built for.
    time_base: ffmpeg::Rational,
    input: (u32, u32, format::Pixel),
    /// The GPU's deinterlacer wouldn't build: its frames come down for
    /// `bwdif`.
    gpu_failed: bool,
    /// The end was sent to the graph.
    flushed: bool,
    /// The graph has given its last frame.
    done: bool,
}

impl Deinterlacer {
    fn new(time_base: ffmpeg::Rational) -> Self {
        Self {
            graph: None,
            time_base,
            input: (0, 0, format::Pixel::None),
            gpu_failed: false,
            flushed: false,
            done: false,
        }
    }

    /// Whether `f` is deinterlaced where it is: in memory, or on the GPU as
    /// NVDEC's (`bwdif_cuda`) and Vulkan's (`bwdif_vulkan`) are where that
    /// filter gives `bwdif`'s frames of their layout
    /// ([`crate::hw::deinterlace`]). Other frames on the GPU come down for
    /// `bwdif`.
    fn takes(&self, f: &frame::Video) -> bool {
        match f.format() {
            format::Pixel::CUDA | format::Pixel::VULKAN => {
                !self.gpu_failed && crate::hw::deinterlace::gives_bwdifs(f)
            }
            _ => !crate::hw::on_gpu(f),
        }
    }

    /// The graph for frames like `f` ([`deinterlacing`]).
    fn build(&mut self, f: &frame::Video) -> Result<()> {
        self.graph = Some(deinterlacing(f, self.time_base)?);
        self.input = (f.width(), f.height(), f.format());
        Ok(())
    }

    /// Feeds the next decoded frame, or the end of the stream (`None`): in
    /// memory, or on the GPU where it is deinterlaced there ([`takes`](Self::takes)).
    fn push(&mut self, f: Option<&frame::Video>) -> Result<()> {
        let Some(f) = f else {
            if let Some(g) = self.graph.as_mut() {
                g.get("in")
                    .expect("source")
                    .source()
                    .flush()
                    .ctx(|| "flushing the deinterlacer".into())?;
            } else {
                self.done = true;
            }
            self.flushed = true;
            return Ok(());
        };
        let down;
        let f = if self.takes(f) {
            f
        } else {
            down = crate::hw::download(f)?;
            &down
        };
        if self.graph.is_none() || self.input != (f.width(), f.height(), f.format()) {
            // A new size or format starts over (losing one frame of context).
            match self.build(f) {
                Err(e) if crate::hw::on_gpu(f) => {
                    crate::debug!(
                        "decoding",
                        "{:?} frames come down to be deinterlaced: {} won't build: {e}",
                        f.format(),
                        deinterlacer(f.format())
                    );
                    self.gpu_failed = true;
                    return self.push(Some(f));
                }
                built => built?,
            }
        }
        self.graph
            .as_mut()
            .expect("built")
            .get("in")
            .expect("source")
            .source()
            .add(f)
            .ctx(|| "deinterlacing".into())
    }

    /// The next progressive frame, if one is ready.
    fn pull(&mut self) -> Result<Option<frame::Video>> {
        let Some(g) = self.graph.as_mut() else {
            return Ok(None);
        };
        let mut sink = g.get("out").expect("sink");
        let mut out = frame::Video::empty();
        match sink.sink().frame(&mut out) {
            Ok(()) => {
                // bwdif halves the time base (for field rates): back to the
                // stream's.
                let tb = sink.sink().time_base();
                let pts = out.pts().map(|p| unsafe {
                    ffmpeg::ffi::av_rescale_q(p, tb.into(), self.time_base.into())
                });
                out.set_pts(pts);
                Ok(Some(out))
            }
            Err(ffmpeg::Error::Eof) => {
                self.done = true;
                Ok(None)
            }
            Err(ffmpeg::Error::Other {
                errno: ffmpeg::util::error::EAGAIN,
            }) => Ok(None),
            Err(e) => Err(Error::Ffmpeg {
                context: "deinterlacing".into(),
                source: e,
            }),
        }
    }
}

/// A decoder for `id` that can decode on `kind`'s GPU: FFmpeg's own, whose
/// frames in GPU memory come through `get_format` like any other (not a
/// library's such as libdav1d, which decodes on the CPU only, nor a wrapper
/// of a GPU's own decoder such as `_cuvid` or `_qsv`). FFmpeg's version
/// isn't asked: a decoder without a configuration for the GPU's device
/// (ProRes's for Vulkan before FFmpeg 8.1) is none.
pub(crate) fn gpu_decoder(id: codec::Id, kind: crate::hw::Kind) -> Option<ffmpeg::Codec> {
    use ffmpeg::ffi;
    let id: ffi::AVCodecID = id.into();
    let mut opaque = std::ptr::null_mut();
    // SAFETY: FFmpeg's codec list and each codec's hardware configs are
    // static; the iteration ends with a null codec, the configs with null.
    unsafe {
        loop {
            let c = ffi::av_codec_iterate(&mut opaque);
            if c.is_null() {
                return None;
            }
            let wrapper = (*c).capabilities & ffi::AV_CODEC_CAP_HARDWARE as i32 != 0;
            if (*c).id != id || ffi::av_codec_is_decoder(c) == 0 || wrapper {
                continue;
            }
            for i in 0.. {
                let cfg = ffi::avcodec_get_hw_config(c, i);
                if cfg.is_null() {
                    break;
                }
                let by_device = ffi::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32;
                if (*cfg).device_type == kind.device_type() && (*cfg).methods & by_device != 0 {
                    return Some(ffmpeg::Codec::wrap(c));
                }
            }
        }
    }
}

/// Picks the frames of the decoder's GPU (its device's kind) when the
/// decoder offers them, else FFmpeg's choice (decoding on the CPU), which
/// `open_on` refuses.
unsafe extern "C" fn gpu_format(
    ctx: *mut ffmpeg::ffi::AVCodecContext,
    formats: *const ffmpeg::ffi::AVPixelFormat,
) -> ffmpeg::ffi::AVPixelFormat {
    // SAFETY: the context's device, set before it opened; FFmpeg's list
    // ends with AV_PIX_FMT_NONE.
    unsafe {
        let wanted = crate::hw::Kind::of_device((*ctx).hw_device_ctx).map(|k| k.pixel().into());
        let mut p = formats;
        while *p != ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_NONE {
            if Some(*p) == wanted {
                return *p;
            }
            p = p.add(1);
        }
        ffmpeg::ffi::avcodec_default_get_format(ctx, formats)
    }
}

/// Whether the H.264 video of `source` has open GOPs: a keyframe that isn't
/// IDR, so decoding from it leans on the GOP before. Reads keyframes at a
/// few points through the file, not the whole file (`keyframes_not_idr`).
/// Open when they don't tell: the CPU decodes either right. An answer they
/// give is kept for each file (by path, size and time).
pub fn h264_open_gop(source: &Input) -> bool {
    remembered(source, |s| keyframes_not_idr(s).ok().flatten())
}

/// The answer `scan` gives for `source`, kept for each file (by path, size
/// and time). Without one it says open and scans again the next time. The
/// scan runs outside the lock, so one file's doesn't hold up another's. Two
/// threads that scan the same file both answer with the one stored first.
fn remembered(source: &Input, scan: impl FnOnce(&Input) -> Option<bool>) -> bool {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static KNOWN: Mutex<Option<HashMap<String, bool>>> = Mutex::new(None);
    let known = || KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    let meta = std::fs::metadata(source.path()).ok();
    let mtime = meta.as_ref().and_then(|m| m.modified().ok());
    let key = format!("{source:?}|{:?}|{mtime:?}", meta.map(|m| m.len()));
    let kept = known().as_ref().and_then(|k| k.get(&key).copied());
    if let Some(open) = kept {
        return open;
    }
    let Some(open) = scan(source) else {
        return true;
    };
    *known()
        .get_or_insert_with(HashMap::new)
        .entry(key)
        .or_insert(open)
}

/// What [`h264_open_gop`] reads from the file. Open when a keyframe isn't
/// IDR. Closed when the keyframes it reads are IDR and one of them comes
/// after the first, or when the first is the file's only one. `None` when
/// they don't tell: a seek or a read fails, or no keyframe after the first
/// turns up.
///
/// It reads the first keyframe and the next one, then the first keyframe
/// from each fifth of the way: by time, or by bytes in a file without a
/// duration (raw H.264). A seek that lands on the first keyframe again
/// tells nothing.
fn keyframes_not_idr(source: &Input) -> Result<Option<bool>> {
    let mut input = source.open()?;
    let Some(stream) = input.streams().best(media::Type::Video) else {
        return Ok(None);
    };
    let index = stream.index();
    // MP4's and Matroska's NAL units come after their lengths (avcC's
    // size of them), others' after start codes.
    // SAFETY: the stream's parameters, read only.
    let lengths = unsafe {
        let p = &*stream.parameters().as_ptr();
        (p.extradata_size > 4 && *p.extradata == 1)
            .then(|| usize::from(*p.extradata.add(4) & 3) + 1)
    };
    let open = |p: &ffmpeg::Packet| not_idr(p.data().unwrap_or_default(), lengths);
    // The demuxer skips the other streams' packets.
    crate::export::audio::discard_others(&input, index);
    let mut packet = ffmpeg::Packet::empty();
    if !matches!(next_keyframe(&mut input, index, &mut packet)?, Next::Key) {
        return Ok(None);
    }
    if open(&packet) {
        return Ok(Some(true));
    }
    let first = packet.position();
    let mut told = match next_keyframe(&mut input, index, &mut packet)? {
        Next::Key if open(&packet) => return Ok(Some(true)),
        Next::Key => true,
        Next::End => return Ok(Some(false)),
        Next::Far => false,
    };
    let duration = input.duration();
    // Seeks take times from the file's start, which a recording may have
    // hours in.
    // SAFETY: the opened context's own field, read only.
    let start = match unsafe { (*input.as_ptr()).start_time } {
        ffmpeg::ffi::AV_NOPTS_VALUE => 0,
        t => t,
    };
    let size = input.io_size().filter(|&s| s > 0);
    for k in 1..5 {
        let sought = if duration > 0 {
            let ts = start + duration * k / 5;
            input.seek(ts, ..ts).is_ok()
        } else if let Some(size) = size {
            // SAFETY: the opened context. FFmpeg keeps the position within
            // the file.
            unsafe {
                ffmpeg::ffi::av_seek_frame(
                    input.as_mut_ptr(),
                    -1,
                    size * k / 5,
                    ffmpeg::ffi::AVSEEK_FLAG_BYTE,
                ) >= 0
            }
        } else {
            false
        };
        if !sought {
            return Ok(None);
        }
        if let Next::Key = next_keyframe(&mut input, index, &mut packet)? {
            if open(&packet) {
                return Ok(Some(true));
            }
            told |= packet.position() > first;
        }
    }
    Ok(told.then_some(false))
}

/// What [`next_keyframe`] finds.
enum Next {
    /// A keyframe, in the packet.
    Key,
    /// The end of the file.
    End,
    /// Neither, in 1000 packets.
    Far,
}

/// Reads `input` on to the next keyframe of stream `index`, into `packet`
/// ([`Next`]).
fn next_keyframe(
    input: &mut format::context::Input,
    index: usize,
    packet: &mut ffmpeg::Packet,
) -> Result<Next> {
    for _ in 0..1000 {
        match packet.read(input) {
            Ok(()) if packet.stream() == index && packet.is_key() => return Ok(Next::Key),
            Ok(()) => {}
            Err(ffmpeg::Error::Eof) => return Ok(Next::End),
            Err(e) => {
                return Err(Error::Ffmpeg {
                    context: "reading packets".into(),
                    source: e,
                });
            }
        }
    }
    Ok(Next::Far)
}

/// Whether the H.264 picture `data` is one that isn't IDR: it has a slice
/// of such a picture (NAL type 1) and none of an IDR one (5). Its NAL units
/// come after their lengths in `lengths` bytes, or after start codes.
fn not_idr(data: &[u8], lengths: Option<usize>) -> bool {
    let mut types = Vec::new();
    match lengths {
        Some(n) => {
            let mut at = 0;
            while let Some(len) = data.get(at..at + n) {
                let Some(&header) = data.get(at + n) else {
                    break;
                };
                types.push(header & 0x1f);
                at += n + len.iter().fold(0, |l, &b| l << 8 | usize::from(b));
            }
        }
        None => {
            for w in data.windows(4) {
                if w[..3] == [0, 0, 1] {
                    types.push(w[3] & 0x1f);
                }
            }
        }
    }
    types.contains(&1) && !types.contains(&5)
}

/// The kind of `gpu`'s decoders that decodes `id`, its device and FFmpeg's
/// decoder for it ([`gpu_decoder`]): its own, or Vulkan's on the same GPU
/// (`GpuDecoder::kind_for`). An error says why the CPU decodes it: `gpu`
/// leaves it to the CPU (`GpuDecoder::why_not`, told by `open_gop` whether
/// the stream has open GOPs), or FFmpeg has no such decoder.
fn on_gpu(
    id: codec::Id,
    gpu: &crate::hw::GpuDecoder,
    open_gop: impl FnOnce() -> bool,
) -> Result<(crate::hw::Kind, crate::hw::Device, ffmpeg::Codec)> {
    if let Some(why) = gpu.why_not(id, open_gop) {
        return Err(Error::Invalid(why.into()));
    }
    let found = gpu
        .kind_for(id)
        .and_then(|kind| Some((kind, gpu_decoder(id, kind)?)));
    let Some((kind, codec)) = found else {
        return Err(Error::Invalid(format!(
            "FFmpeg has no {} decoder for {}",
            gpu.kind().label(),
            id.name()
        )));
    };
    Ok((kind, gpu.device_for(kind)?, codec))
}

/// Opens the video decoder of `ctx` for `id` on `gpu`, its frames in GPU
/// memory, with the decoder [`on_gpu`] picks. On the CPU, FFmpeg's default,
/// where there is no `gpu` or `on_gpu` finds none. A stream the GPU won't
/// take decodes on the CPU all the same: `get_format` falls back to it.
pub(crate) fn open_video(
    mut ctx: codec::context::Context,
    id: codec::Id,
    gpu: Option<&crate::hw::GpuDecoder>,
    open_gop: impl FnOnce() -> bool,
) -> Result<ffmpeg::decoder::Video> {
    let Some((_, device, codec)) = gpu.and_then(|g| on_gpu(id, g, open_gop).ok()) else {
        return ctx.decoder().video().ctx(|| "opening video decoder".into());
    };
    // SAFETY: the context isn't opened yet; it takes a reference of its own.
    unsafe {
        let raw = ctx.as_mut_ptr();
        let hw = ffmpeg::ffi::av_buffer_ref(device.0);
        if hw.is_null() {
            return Err(Error::Invalid("no memory for a GPU device".into()));
        }
        (*raw).hw_device_ctx = hw;
        (*raw).get_format = Some(gpu_format);
        // Frames held on their way to be measured.
        (*raw).extra_hw_frames = 8;
        // The GPU decodes; frame threads would only add latency.
        (*raw).thread_count = 1;
    }
    ctx.decoder()
        .open_as(codec)
        .and_then(|d| d.video())
        .ctx(|| "opening video decoder".into())
}

/// `f` in memory: copied down when it is in GPU memory.
pub(crate) fn in_memory(f: frame::Video) -> Result<frame::Video> {
    if crate::hw::on_gpu(&f) {
        crate::hw::download(&f)
    } else {
        Ok(f)
    }
}

/// Where `f` was decoded, for `VIDCUT_DEBUG`: on `gpu` (its decoder of
/// `f`'s kind), or the CPU.
pub(crate) fn decoded_on(f: &frame::Video, gpu: Option<&crate::hw::GpuDecoder>) -> String {
    match (gpu, crate::hw::Kind::of_frame(f)) {
        (Some(gpu), Some(kind)) => gpu.labeled(kind),
        _ => "the CPU".into(),
    }
}

/// Facts carried by a decoded frame rather than the stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameProps {
    /// Interlaced, with the top field shown first (`Some(true)`) or the
    /// bottom one (`Some(false)`); `None` when progressive.
    pub top_field_first: Option<bool>,
    pub transfer: ffmpeg::ffi::AVColorTransferCharacteristic,
    /// Its HDR metadata (the stream's too: decoders put the container's on
    /// every frame).
    pub hdr: crate::hdr::Metadata,
}

impl FrameProps {
    pub fn of(frame: &frame::Video) -> Self {
        let f = unsafe { &*frame.as_ptr() };
        let interlaced = f.flags & ffmpeg::ffi::AV_FRAME_FLAG_INTERLACED != 0;
        let top = f.flags & ffmpeg::ffi::AV_FRAME_FLAG_TOP_FIELD_FIRST != 0;
        Self {
            top_field_first: interlaced.then_some(top),
            transfer: f.color_trc,
            hdr: crate::hdr::of_frame(frame),
        }
    }
}

/// Clockwise rotation (0, 90, 180 or 270) that shows a stream's stored
/// pictures upright, from its display matrix (phones write one).
pub fn stream_rotation(params: &codec::Parameters) -> u16 {
    unsafe {
        let p = &*params.as_ptr();
        let sd = ffmpeg::ffi::av_packet_side_data_get(
            p.coded_side_data,
            p.nb_coded_side_data,
            ffmpeg::ffi::AVPacketSideDataType::AV_PKT_DATA_DISPLAYMATRIX,
        );
        if sd.is_null() || (*sd).size < 9 * 4 {
            return 0;
        }
        // FFmpeg gives the counterclockwise angle.
        let ccw = ffmpeg::ffi::av_display_rotation_get((*sd).data as *const i32);
        if !ccw.is_finite() {
            return 0;
        }
        ((-ccw).rem_euclid(360.0) / 90.0).round() as u16 % 4 * 90
    }
}

impl VideoReader {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::File(path.to_path_buf()))
    }

    pub fn open_input(source: &Input) -> Result<Self> {
        let mut reader = Self::open_with(source, None)?;
        reader.opened(None);
        Ok(reader)
    }

    /// Decoding on `gpu`: frames stay in its memory (`Pixel::CUDA`, `VAAPI`,
    /// `D3D11`, `VULKAN` for ProRes beside NVDEC), the same frames with the
    /// same timestamps as decoding on the CPU. Interlaced ones are
    /// deinterlaced as the CPU reader deinterlaces them: on the GPU for
    /// NVDEC's and Vulkan's where its filter gives `bwdif`'s frames, in
    /// memory for the others, which come down for it. Fails when `gpu`
    /// can't decode the stream.
    pub fn open_on(source: &Input, gpu: &crate::hw::GpuDecoder) -> Result<Self> {
        let mut reader = Self::open_with(source, Some(gpu))?;
        let pixel = reader.kind.map(crate::hw::Kind::pixel);
        match reader.pending.as_ref().map(|f| f.format()) {
            Some(p) if Some(p) == pixel => {
                reader.opened(None);
                Ok(reader)
            }
            _ => Err(Error::Invalid(
                "FFmpeg decoded it in memory: the GPU's decoder wouldn't take the stream \
                 (VIDCUT_AV_LOG=verbose shows FFmpeg's reason)"
                    .into(),
            )),
        }
    }

    /// Decoding on `gpu` when it decodes the stream (frames in its
    /// memory), on the CPU otherwise (frames in memory): ProRes, VP8, a
    /// profile the GPU lacks.
    pub fn open_gpu_or_cpu(source: &Input, gpu: &crate::hw::GpuDecoder) -> Result<Self> {
        Self::open_on(source, gpu).or_else(|e| {
            let mut reader = Self::open_with(source, None)?;
            reader.opened(Some(&format!("not on {gpu}: {e}")));
            Ok(reader)
        })
    }

    /// What the file holds and what decodes it, for people: the codec,
    /// profile and rate, FFmpeg's decoder and where it runs, and why that
    /// is the CPU when a GPU was asked.
    pub fn decoding(&self) -> &Arc<Decoding> {
        &self.decoding
    }

    /// Notes what the file holds and what decodes it ([`decoding`](Self::decoding)),
    /// `why` when that is the CPU though a GPU was asked, and says so and
    /// what the first frame is under `VIDCUT_DEBUG`.
    fn opened(&mut self, why: Option<&str>) {
        // SAFETY: the opened decoder's context, read only.
        let (id, profile) = unsafe {
            let ctx = &*self.decoder.as_ptr();
            (ctx.codec_id, ctx.profile)
        };
        self.decoding = Arc::new(Decoding {
            codec: codec::Id::from(id).name().to_string(),
            profile: c_name(unsafe { ffmpeg::ffi::avcodec_profile_name(id, profile) }),
            fps: self
                .frame_rate()
                .map(|r| f64::from(r.numerator()) / f64::from(r.denominator())),
            interlaced: self.interlaced,
            rotation: self.rotation,
            decoder: self
                .decoder
                .codec()
                .map_or("?".into(), |c| c.name().to_string()),
            gpu: self.gpu.as_ref().zip(self.kind).map(|(g, k)| g.labeled(k)),
            why: why.map(String::from),
        });
        crate::debug!(
            "decoding",
            "{}, {}: {}; first frame {}",
            std::thread::current().name().unwrap_or("a thread"),
            self.path().display(),
            self.decoding,
            self.first_frame()
        );
    }

    /// The first frame's size, layout, colors and memory, as FFmpeg names
    /// them.
    fn first_frame(&self) -> String {
        match self.pending.as_ref() {
            Some(f) => {
                // SAFETY: FFmpeg's names of a frame's own tags.
                let (layout, matrix, range, transfer) = unsafe {
                    let raw = &*f.as_ptr();
                    (
                        c_name(ffmpeg::ffi::av_get_pix_fmt_name(
                            crate::hw::sw_format(f).into(),
                        )),
                        c_name(ffmpeg::ffi::av_color_space_name(raw.colorspace)),
                        c_name(ffmpeg::ffi::av_color_range_name(raw.color_range)),
                        c_name(ffmpeg::ffi::av_color_transfer_name(raw.color_trc)),
                    )
                };
                let unknown = || "unknown".into();
                let at = if crate::hw::on_gpu(f) {
                    "GPU memory"
                } else {
                    "memory"
                };
                format!(
                    "{}x{} {}, matrix {}, {} range, transfer {}, in {at}",
                    f.width(),
                    f.height(),
                    layout.unwrap_or_else(unknown),
                    matrix.unwrap_or_else(unknown),
                    range.unwrap_or_else(unknown),
                    transfer.unwrap_or_else(unknown),
                )
            }
            None => "none".into(),
        }
    }

    fn open_with(source: &Input, device: Option<&crate::hw::GpuDecoder>) -> Result<Self> {
        let mut reader = Self::open_raw(source, device)?;
        // Learn `o` up front; keep the frame as look-ahead so reading from the
        // start still returns it.
        reader.pending = reader.decode_one()?;
        reader.first_pts = reader.pending.as_ref().and_then(|f| f.pts());
        reader.first_props = reader.pending.as_ref().map(FrameProps::of);
        // Interlaced by the stream's field order or the first frame's flags.
        use ffmpeg::ffi::AVFieldOrder as F;
        let field_order = unsafe {
            (*reader
                .input
                .stream(reader.stream)
                .expect("found")
                .parameters()
                .as_ptr())
            .field_order
        };
        let interlaced = matches!(
            field_order,
            F::AV_FIELD_TT | F::AV_FIELD_BB | F::AV_FIELD_TB | F::AV_FIELD_BT
        ) || reader
            .first_props
            .is_some_and(|p| p.top_field_first.is_some());
        reader.interlaced = interlaced;
        if interlaced {
            reader.deinterlacer = Some(Deinterlacer::new(reader.time_base));
        }
        Ok(reader)
    }

    fn open_raw(source: &Input, device: Option<&crate::hw::GpuDecoder>) -> Result<Self> {
        let path = source.path();
        let input = source.open()?;
        let stream = input
            .streams()
            .best(media::Type::Video)
            .ok_or_else(|| Error::Invalid(format!("{}: no video stream", path.display())))?;
        let index = stream.index();
        let time_base = stream.time_base();
        let start_pts = Some(stream.start_time()).filter(|&t| t != ffmpeg::ffi::AV_NOPTS_VALUE);
        let rotation = stream_rotation(&stream.parameters());
        let mut ctx = codec::context::Context::from_parameters(stream.parameters())
            .ctx(|| "creating video decoder context".into())?;
        // The packet time base lets the decoder keep timestamps right when it
        // drops samples or frames (FFmpeg's CLI sets it too).
        unsafe { (*ctx.as_mut_ptr()).pkt_timebase = time_base.into() };
        let codec_id = stream.parameters().id();
        // The GPU's decoder for the stream: its own, or Vulkan's for ProRes
        // beside NVDEC. FFmpeg's default decoder may decode on the CPU only
        // (libdav1d for AV1): one that can use the GPU.
        let on = match device {
            Some(d) => Some(on_gpu(codec_id, d, || h264_open_gop(source))?),
            None => None,
        };
        match &on {
            // The GPU decodes; frame threads would only add latency.
            Some((_, device, _)) => unsafe {
                let raw = ctx.as_mut_ptr();
                let hw = ffmpeg::ffi::av_buffer_ref(device.0);
                if hw.is_null() {
                    return Err(Error::Invalid("no memory for a GPU device".into()));
                }
                (*raw).hw_device_ctx = hw;
                (*raw).get_format = Some(gpu_format);
                // Frames a filter graph and the encoder hold on to.
                (*raw).extra_hw_frames = 8;
                (*raw).thread_count = 1;
            },
            None => ctx.set_threading(codec::threading::Config {
                kind: codec::threading::Type::Frame,
                count: 0, // let FFmpeg choose
            }),
        }
        let decoder = match &on {
            Some((_, _, codec)) => ctx.decoder().open_as(*codec).and_then(|d| d.video()),
            None => ctx.decoder().video(),
        };
        let decoder = crate::parts::decoder_opened(decoder, codec_id, "video")?;
        crate::export::audio::discard_others(&input, index);
        Ok(Self {
            source: source.clone(),
            input,
            stream: index,
            time_base,
            start_pts,
            decoder,
            draining: false,
            min_pts: None,
            pending: None,
            packet: None,
            first_pts: None,
            rotation,
            first_props: None,
            deinterlacer: None,
            interlaced: false,
            gpu: device.cloned(),
            kind: on.map(|(kind, ..)| kind),
            decoding: Arc::default(),
        })
    }

    /// The GPU decoder decoding, when one does: its frames are in its
    /// memory. Vulkan's for ProRes beside NVDEC.
    pub fn gpu(&self) -> Option<crate::hw::Kind> {
        self.kind
    }

    /// The CUDA device of the GPU decoder it decodes with, whichever of its
    /// kinds decodes the stream: NVDEC's, and for ProRes on Vulkan, the one
    /// Vulkan's device was made from. Pictures made of its frames for NVENC
    /// go there.
    pub(crate) fn decoders_cuda(&self) -> Option<&crate::cuda::CudaDevice> {
        self.gpu
            .as_ref()
            .filter(|_| self.kind.is_some())
            .and_then(crate::hw::GpuDecoder::cuda)
    }

    /// The stream's frames are interlaced (and deinterlaced here, on the
    /// GPU when decoding on NVDEC or Vulkan).
    pub fn interlaced(&self) -> bool {
        self.interlaced
    }

    /// The next frame [`next_frame`](Self::next_frame) returns, right after
    /// opening: the first one.
    pub fn peek(&self) -> Option<&frame::Video> {
        self.pending.as_ref()
    }

    /// Whether the frames it gives are where it decodes them, as the first
    /// one ([`peek`](Self::peek)) is. An interlaced stream's come down to
    /// be deinterlaced where its deinterlacer doesn't take them there
    /// ([`Deinterlacer::takes`]): other GPUs', and NVDEC's and Vulkan's
    /// where their filter doesn't give `bwdif`'s frames.
    pub(crate) fn frames_stay(&self) -> bool {
        match (&self.deinterlacer, &self.pending) {
            (Some(d), Some(first)) => d.takes(first),
            _ => true,
        }
    }

    /// Time base of frame timestamps.
    pub fn time_base(&self) -> ffmpeg::Rational {
        self.time_base
    }

    /// The file this reads.
    /// The file, or the sequence's pattern.
    pub fn path(&self) -> &Path {
        self.source.path()
    }

    pub fn input(&self) -> &Input {
        &self.source
    }

    /// Clockwise rotation (0, 90, 180 or 270) that shows the pictures upright.
    pub fn rotation(&self) -> u16 {
        self.rotation
    }

    /// The stream's average frame rate, when the container knows it.
    pub fn frame_rate(&self) -> Option<ffmpeg::Rational> {
        let r = self.input.stream(self.stream)?.avg_frame_rate();
        (r.numerator() > 0 && r.denominator() > 0).then_some(r)
    }

    /// Fields and color of the first decodable frame.
    pub fn first_props(&self) -> Option<FrameProps> {
        self.first_props
    }

    /// Stream start timestamp from the container, if known.
    pub fn start_pts(&self) -> Option<i64> {
        self.start_pts
    }

    /// Pts of the first decodable frame, if the stream has any frames.
    pub fn first_pts(&self) -> Option<i64> {
        self.first_pts
    }

    /// Pts of the frame decoded and not yet returned: right after a
    /// [`seek`](Self::seek), the keyframe decoding started from.
    pub fn pending_pts(&self) -> Option<i64> {
        self.pending.as_ref().and_then(|f| f.pts())
    }

    /// Frames before this pts are passed over: right after a
    /// [`seek`](Self::seek) its target, or the keyframe it landed on after
    /// [`from_keyframe`](Self::from_keyframe).
    pub fn passes_before(&self) -> Option<i64> {
        self.min_pts
    }

    /// After a [`seek`](Self::seek): the frames from the keyframe it
    /// landed on come next, not only those from its target, so a reader
    /// that looked for a keyframe decodes on from it instead of seeking
    /// again.
    pub fn from_keyframe(&mut self) {
        if let Some(pts) = self.pending_pts() {
            self.min_pts = Some(pts);
        }
    }

    /// Returns the next frame in presentation order, or `None` at end of stream.
    /// The frame's pts is its best-effort timestamp.
    pub fn next_frame(&mut self) -> Result<Option<frame::Video>> {
        loop {
            let Some(frame) = self.next_progressive()? else {
                return Ok(None);
            };
            match (self.min_pts, frame.pts()) {
                (Some(min), Some(pts)) if pts < min => continue,
                _ => return Ok(Some(frame)),
            }
        }
    }

    /// The next decoded frame in presentation order, deinterlaced when the
    /// stream is interlaced.
    fn next_progressive(&mut self) -> Result<Option<frame::Video>> {
        if self.deinterlacer.is_none() {
            return match self.pending.take() {
                Some(f) => Ok(Some(f)),
                None => self.decode_one(),
            };
        }
        loop {
            let d = self.deinterlacer.as_mut().expect("interlaced");
            if let Some(f) = d.pull()? {
                return Ok(Some(f));
            }
            if d.done || d.flushed {
                // After the end was sent, nothing more is coming.
                d.done = true;
                return Ok(None);
            }
            let frame = match self.pending.take() {
                Some(f) => Some(f),
                None => self.decode_one()?,
            };
            let d = self.deinterlacer.as_mut().expect("interlaced");
            d.push(frame.as_ref())?;
        }
    }

    /// Positions the reader so the next frame returned is the first one with
    /// `pts >= target` (in [`time_base`](Self::time_base) units).
    pub fn seek(&mut self, target: i64) -> Result<()> {
        // Frames around the new position are another stretch: the
        // deinterlacer starts over.
        if self.deinterlacer.is_some() {
            self.deinterlacer = Some(Deinterlacer::new(self.time_base));
        }
        // What it held is from before: a seek cut short (cancelled) leaves
        // none of it to read on from, its position unknown.
        self.pending = None;
        self.min_pts = None;
        // Seek to a keyframe at or before the target. Demuxers without an index
        // (e.g. MPEG-TS) may land after it, so back off until the first decoded
        // frame is not past the target. Targets before the first frame, or
        // back-offs beyond the start, reopen the file instead (for the same
        // decoder, flushed): that is the only rewind every demuxer gets right.
        let one_second = (i64::from(self.time_base.denominator())
            / i64::from(self.time_base.numerator().max(1)))
        .max(1);
        let first = self.first_pts.unwrap_or(i64::MIN);
        let mut back = 0i64;
        loop {
            crate::cancel::stop()?;
            let ts = target.saturating_sub(back);
            let at_start = ts <= first
                || back > 3600 * one_second
                || unsafe {
                    ffmpeg::ffi::avformat_seek_file(
                        self.input.as_mut_ptr(),
                        self.stream as i32,
                        i64::MIN,
                        ts,
                        ts,
                        0,
                    )
                } < 0;
            self.packet = None;
            if !at_start {
                // A demuxer that seeks by decode times (MP4) lands on a
                // keyframe shown after the target when the target lies
                // just before it: seen in its first packet, the seek goes
                // on to before that keyframe without decoding (FFmpeg 8.0
                // makes a GPU's H.264 decoder anew at each flush).
                let packet = self.first_packet()?;
                if let Some(p) = &packet
                    && p.is_key()
                    && p.pts().is_some_and(|pts| pts > target)
                    && let Some(dts) = p.dts()
                    && dts <= ts
                {
                    back = back.max(target - dts + 1);
                    continue;
                }
                self.packet = packet;
            }
            if at_start {
                self.rewind()?;
            } else {
                self.decoder.flush();
                self.draining = false;
            }
            self.min_pts = None;
            self.pending = None;
            let frame = self.decode_one()?;
            // Overshot if the first decodable frame is after the target, or if
            // nothing decodes at all (landed after the last keyframe; decoders
            // drop frames until they see one).
            let past_target = match frame.as_ref() {
                Some(f) => f.pts().is_some_and(|p| p > target),
                None => true,
            };
            if past_target && !at_start {
                back = if back == 0 { one_second } else { back * 2 };
                continue;
            }
            self.pending = frame;
            self.min_pts = Some(target);
            return Ok(());
        }
    }

    /// The stream's next packet, read and not sent; `None` at the end.
    fn first_packet(&mut self) -> Result<Option<ffmpeg::Packet>> {
        loop {
            crate::cancel::stop()?;
            let mut packet = ffmpeg::Packet::empty();
            match packet.read(&mut self.input) {
                Ok(()) if packet.stream() == self.stream => return Ok(Some(packet)),
                Ok(()) => continue,
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(e) => {
                    return Err(Error::Ffmpeg {
                        context: "reading packet".into(),
                        source: e,
                    });
                }
            }
        }
    }

    /// Reopens the input at its very beginning, for the decoder it has,
    /// flushed: a decoder made anew would start a pool of frames of its own
    /// while the kept pictures of this one hold its pool (gotchas.md).
    fn rewind(&mut self) -> Result<()> {
        let input = self.source.open()?;
        crate::export::audio::discard_others(&input, self.stream);
        self.input = input;
        self.decoder.flush();
        self.draining = false;
        Ok(())
    }

    /// Decodes the next frame from the stream, feeding packets as needed.
    fn decode_one(&mut self) -> Result<Option<frame::Video>> {
        let mut damaged = 0;
        loop {
            crate::cancel::stop()?;
            crate::first::wait()?;
            let mut frame = frame::Video::empty();
            match self.decoder.receive_frame(&mut frame) {
                Ok(()) => {
                    let pts = frame.timestamp();
                    frame.set_pts(pts);
                    DECODED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return Ok(Some(frame));
                }
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(ffmpeg::Error::Other {
                    errno: ffmpeg::util::error::EAGAIN,
                }) => {}
                // A damaged frame: on to the next.
                Err(ffmpeg::Error::InvalidData) if damaged < DAMAGED_IN_A_ROW => {
                    damaged += 1;
                    continue;
                }
                Err(e) => {
                    return Err(Error::Ffmpeg {
                        context: "decoding video".into(),
                        source: e,
                    });
                }
            }
            if self.draining {
                return Ok(None);
            }
            if let Some(packet) = self.packet.take() {
                send_packet(&mut self.decoder, &packet)?;
                continue;
            }
            loop {
                crate::cancel::stop()?;
                let mut packet = ffmpeg::Packet::empty();
                match packet.read(&mut self.input) {
                    Ok(()) if packet.stream() == self.stream => {
                        send_packet(&mut self.decoder, &packet)?;
                        break;
                    }
                    Ok(()) => continue,
                    Err(ffmpeg::Error::Eof) => {
                        self.decoder.send_eof().ctx(|| "flushing decoder".into())?;
                        self.draining = true;
                        break;
                    }
                    Err(e) => {
                        return Err(Error::Ffmpeg {
                            context: "reading packet".into(),
                            source: e,
                        });
                    }
                }
            }
        }
    }
}

/// Reads the [`barcode`](crate::barcode) from a decoded frame of any pixel format.
pub fn read_barcode(frame: &frame::Video) -> Result<Option<u32>> {
    use format::Pixel;
    let (w, h) = (frame.width() as usize, frame.height() as usize);
    match frame.format() {
        // 8-bit formats whose plane 0 is luma.
        Pixel::YUV420P
        | Pixel::YUV422P
        | Pixel::YUV444P
        | Pixel::YUVJ420P
        | Pixel::YUVJ422P
        | Pixel::YUVJ444P
        | Pixel::NV12
        | Pixel::GRAY8 => Ok(crate::barcode::read(frame.data(0), w, h, frame.stride(0))),
        other => {
            // Anything else (10-bit, RGB, hardware-downloaded formats): convert to gray.
            let mut scaler = scaling::Context::get(
                other,
                w as u32,
                h as u32,
                Pixel::GRAY8,
                w as u32,
                h as u32,
                scaling::Flags::POINT,
            )
            .ctx(|| format!("creating {other:?} -> gray converter"))?;
            let mut gray = frame::Video::empty();
            scaler
                .run(frame, &mut gray)
                .ctx(|| "converting frame to gray".into())?;
            Ok(crate::barcode::read(gray.data(0), w, h, gray.stride(0)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Input, not_idr, remembered};
    use std::sync::{Arc, Barrier, mpsc};

    /// [`remembered`] of the file `name` on a thread of its own, its scan
    /// held: the barrier is passed once the scan has started, and again to
    /// let it answer `open`.
    fn held(name: &str, open: bool) -> (std::thread::JoinHandle<bool>, Arc<Barrier>) {
        let barrier = Arc::new(Barrier::new(2));
        let at = barrier.clone();
        let source = Input::File(name.into());
        let thread = std::thread::spawn(move || {
            remembered(&source, |_| {
                at.wait();
                at.wait();
                Some(open)
            })
        });
        barrier.wait();
        (thread, barrier)
    }

    /// What [`remembered`] of the file `name`, scanned as `open`, says on
    /// another thread within 30 s.
    fn meanwhile(name: &str, open: bool) -> Result<bool, mpsc::RecvTimeoutError> {
        let (tx, rx) = mpsc::channel();
        let source = Input::File(name.into());
        std::thread::spawn(move || tx.send(remembered(&source, |_| Some(open))));
        rx.recv_timeout(std::time::Duration::from_secs(30))
    }

    #[test]
    fn one_files_scan_does_not_hold_up_anothers() {
        let (first, go) = held("held.mp4", false);
        let second = meanwhile("free.mp4", true);
        go.wait();
        first.join().unwrap();
        assert_eq!(second, Ok(true));
    }

    #[test]
    fn the_answer_stored_first_stays() {
        let (late, go) = held("twice.mp4", false);
        let stored = meanwhile("twice.mp4", true);
        go.wait();
        assert_eq!(stored, Ok(true));
        assert!(late.join().unwrap());
    }

    #[test]
    fn a_slice_of_a_picture_not_idr_makes_it_not_idr() {
        // Its length in 4 bytes, then NAL type 1.
        assert!(not_idr(&[0, 0, 0, 2, 0x41, 0x9a], Some(4)));
    }

    #[test]
    fn an_idr_slice_makes_it_idr() {
        assert!(!not_idr(&[0, 0, 0, 2, 0x65, 0x88], Some(4)));
    }

    #[test]
    fn start_codes_lead_to_the_nal_units() {
        // An access unit delimiter (9), then an IDR slice (5).
        assert!(!not_idr(
            &[0, 0, 0, 1, 0x09, 0xf0, 0, 0, 1, 0x65, 0x88],
            None
        ));
    }
}
