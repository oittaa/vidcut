//! Video decoding with frame-exact seeking.
//!
//! Seek semantics match mpv's hr-seek and FFmpeg's accurate seek: after
//! [`VideoReader::seek`] to `target`, the next frame returned is the first frame
//! whose presentation timestamp is `>= target`. Callers put the half-frame bias
//! into `target` (see the time map), so that rounding in timestamps never selects
//! the neighboring frame.

use std::path::{Path, PathBuf};

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
/// With no start, `origin` does (the stream's start, or the first packet's
/// timestamp): counting the length from zero puts an offset clock past the
/// end, a real gap is dropped, and later audio shifts early. No positive
/// duration means the end is unknown.
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

fn io_seekable(input: &format::context::Input) -> bool {
    unsafe {
        let ctx = &*input.as_ptr();
        !ctx.pb.is_null() && ((*ctx.pb).seekable & ffmpeg::ffi::AVIO_SEEKABLE_NORMAL) != 0
    }
}

fn rewind(input: &mut format::context::Input) -> bool {
    unsafe {
        ffmpeg::ffi::avformat_seek_file(input.as_mut_ptr(), -1, i64::MIN, i64::MIN, i64::MIN, 0)
            >= 0
    }
}

/// First timestamp (s) on `stream`, then back to the start of the file.
/// Nothing is read when the input can't be rewound. `None` when no early
/// packet carries a timestamp. A failed rewind also returns `None`: the
/// demuxer is left where the scan stopped, so the length is not placed on
/// a read that no longer starts at the beginning.
fn first_packet_time(input: &mut format::context::Input, stream: usize, tick: f64) -> Option<f64> {
    if !io_seekable(input) {
        return None;
    }
    let mut found = None;
    let mut seen = 0u32;
    let mut scanned = 0u32;
    for (s, packet) in input.packets() {
        scanned += 1;
        // The first timestamp is near the start. Don't read the whole file
        // looking for one that isn't there.
        if scanned > 4096 {
            break;
        }
        if s.index() != stream {
            continue;
        }
        seen += 1;
        if let Some(pts) = packet.pts() {
            found = Some(pts as f64 * tick);
            break;
        }
        if seen >= 32 {
            break;
        }
    }
    if rewind(input) { found } else { None }
}

/// [`file_end`] for an open input.
///
/// `stream_start` is that stream's `start_time` (stream time base) and
/// `tick` its time base in seconds. When the container has a duration but
/// no start, the length is placed on the stream's clock, or on the first
/// packet's timestamp if the stream doesn't say.
pub(crate) fn container_end(
    input: &mut format::context::Input,
    stream: usize,
    stream_start: i64,
    tick: f64,
) -> Option<f64> {
    let (start, duration) = unsafe {
        let ctx = &*input.as_ptr();
        (opt_av_time(ctx.start_time), opt_av_time(ctx.duration))
    };
    let origin = if start.is_none() && duration.is_some_and(|d| d > 0.0) {
        stream_origin(stream_start, tick).or_else(|| first_packet_time(input, stream, tick))
    } else {
        None
    };
    file_end(start, duration, origin)
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
            Input::File(p) => format::input(p).ctx(|| format!("opening {}", p.display())),
            Input::Sequence {
                pattern,
                first,
                rate,
            } => open_sequence(pattern, *first, *rate),
        }
    }
}

fn open_sequence(pattern: &Path, first: u64, rate: (i32, i32)) -> Result<format::context::Input> {
    let what = || format!("opening {}", pattern.display());
    let mut options = ffmpeg::Dictionary::new();
    options.set("pattern_type", "sequence");
    options.set("start_number", &first.to_string());
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
    /// Pts of the first decodable frame (`o` in the time map).
    first_pts: Option<i64>,
    /// Clockwise rotation that shows the pictures upright.
    rotation: u16,
    /// What the first decodable frame says about fields and color.
    first_props: Option<FrameProps>,
    /// For interlaced streams: every decoded frame goes through it.
    deinterlacer: Option<Deinterlacer>,
}

/// `bwdif` over a stream's frames in order: one progressive frame for each
/// interlaced one, made from both its fields with the frames around it
/// (send_frame); progressive frames pass through. Holds one frame back.
struct Deinterlacer {
    graph: Option<filter::Graph>,
    /// The stream's time base, and the parameters the graph was built for.
    time_base: ffmpeg::Rational,
    input: (u32, u32, format::Pixel),
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
            flushed: false,
            done: false,
        }
    }

    fn build(&mut self, f: &frame::Video) -> Result<()> {
        let sar = f.aspect_ratio();
        let args = format!(
            "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect={}/{}",
            f.width(),
            f.height(),
            ffmpeg::ffi::AVPixelFormat::from(f.format()) as i32,
            self.time_base.numerator(),
            self.time_base.denominator(),
            sar.numerator().max(1),
            sar.denominator().max(1)
        );
        let mut graph = filter::Graph::new();
        let find = |name: &str| {
            filter::find(name).ok_or_else(|| Error::Invalid(format!("no {name} filter")))
        };
        graph
            .add(&find("buffer")?, "in", &args)
            .ctx(|| "creating deinterlacer source".into())?;
        graph
            .add(&find("buffersink")?, "out", "")
            .ctx(|| "creating deinterlacer sink".into())?;
        let spec = "bwdif=mode=send_frame:parity=auto:deint=interlaced";
        graph
            .output("in", 0)
            .and_then(|p| p.input("out", 0))
            .and_then(|p| p.parse(spec))
            .ctx(|| format!("building {spec}"))?;
        graph.validate().ctx(|| format!("configuring {spec}"))?;
        self.graph = Some(graph);
        self.input = (f.width(), f.height(), f.format());
        Ok(())
    }

    /// Feeds the next decoded frame, or the end of the stream (`None`).
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
        if self.graph.is_none() || self.input != (f.width(), f.height(), f.format()) {
            // A new size or format starts over (losing one frame of context).
            self.build(f)?;
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

/// Facts carried by a decoded frame rather than the stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameProps {
    /// Interlaced, with the top field shown first (`Some(true)`) or the
    /// bottom one (`Some(false)`); `None` when progressive.
    pub top_field_first: Option<bool>,
    pub transfer: ffmpeg::ffi::AVColorTransferCharacteristic,
}

impl FrameProps {
    pub fn of(frame: &frame::Video) -> Self {
        let f = unsafe { &*frame.as_ptr() };
        let interlaced = f.flags & ffmpeg::ffi::AV_FRAME_FLAG_INTERLACED != 0;
        let top = f.flags & ffmpeg::ffi::AV_FRAME_FLAG_TOP_FIELD_FIRST != 0;
        Self {
            top_field_first: interlaced.then_some(top),
            transfer: f.color_trc,
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
        let mut reader = Self::open_raw(source)?;
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
        if interlaced {
            reader.deinterlacer = Some(Deinterlacer::new(reader.time_base));
        }
        Ok(reader)
    }

    fn open_raw(source: &Input) -> Result<Self> {
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
        ctx.set_threading(codec::threading::Config {
            kind: codec::threading::Type::Frame,
            count: 0, // let FFmpeg choose
        });
        let decoder = ctx
            .decoder()
            .video()
            .ctx(|| "opening video decoder".into())?;
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
            first_pts: None,
            rotation,
            first_props: None,
            deinterlacer: None,
        })
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
        // Seek to a keyframe at or before the target. Demuxers without an index
        // (e.g. MPEG-TS) may land after it, so back off until the first decoded
        // frame is not past the target. Targets before the first frame, or
        // back-offs beyond the start, reopen the file instead: that is the only
        // rewind every demuxer gets right.
        let one_second = (i64::from(self.time_base.denominator())
            / i64::from(self.time_base.numerator().max(1)))
        .max(1);
        let first = self.first_pts.unwrap_or(i64::MIN);
        let mut back = 0i64;
        loop {
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

    /// Reopens the input at its very beginning.
    fn rewind(&mut self) -> Result<()> {
        let fresh = Self::open_raw(&self.source)?;
        self.input = fresh.input;
        self.decoder = fresh.decoder;
        self.draining = false;
        Ok(())
    }

    /// Decodes the next frame from the stream, feeding packets as needed.
    fn decode_one(&mut self) -> Result<Option<frame::Video>> {
        let mut damaged = 0;
        loop {
            let mut frame = frame::Video::empty();
            match self.decoder.receive_frame(&mut frame) {
                Ok(()) => {
                    let pts = frame.timestamp();
                    frame.set_pts(pts);
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
            loop {
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
mod gap_tests {
    use super::{AUDIO_GAP, AUDIO_GAP_BOUND, audio_gap, file_end};

    #[test]
    fn a_missing_duration_does_not_honor_a_far_timestamp() {
        assert_eq!(file_end(None, None, Some(10.0)), None);
        assert_eq!(file_end(Some(0.0), Some(0.0), Some(10.0)), None);
        assert_eq!(file_end(Some(1.0), Some(-1.0), None), None);
        let short = AUDIO_GAP + 0.02;
        assert!((audio_gap(1.0, 1.0 + short, None) - short).abs() < 1e-12);
        // Past the bound: play in order, rather than silence for the timestamp.
        assert_eq!(audio_gap(1.0, 1.0 + AUDIO_GAP_BOUND + 0.001, None), 0.0);
        assert_eq!(audio_gap(0.0, 10_000.0, None), 0.0);
    }

    #[test]
    fn a_missing_start_keeps_duration_on_the_stream_clock() {
        // The length sits at the stream origin, not at zero.
        assert_eq!(file_end(None, Some(10.0), Some(3_600.0)), Some(3_610.0));
        let end = file_end(None, Some(10.0), Some(3_600.0));
        assert!((audio_gap(3_601.0, 3_601.05, end) - 0.05).abs() < 1e-12);
        // A known container start wins over the stream origin.
        assert_eq!(file_end(Some(1.0), Some(10.0), Some(3_600.0)), Some(11.0));
        // The old clock (end = duration from zero) drops that same gap.
        assert_eq!(audio_gap(3_601.0, 3_601.05, Some(10.0)), 0.0);
    }

    #[test]
    fn a_frame_past_a_known_end_is_not_a_gap() {
        assert!(audio_gap(9.0, 9.05, Some(10.0)) > 0.0);
        assert_eq!(audio_gap(9.0, 10.5, Some(10.0)), 0.0);
        // Inside the file, longer than the unknown-end bound, still a gap.
        assert!((audio_gap(0.0, 5.0, Some(10.0)) - 5.0).abs() < 1e-12);
        // Equal to the rounding allowance: not a gap. Adding the constant
        // onto another timestamp is not exact in f64.
        assert_eq!(audio_gap(0.0, AUDIO_GAP, Some(100.0)), 0.0);
    }
}
