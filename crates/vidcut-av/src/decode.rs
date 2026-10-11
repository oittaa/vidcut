//! Video decoding with frame-exact seeking.
//!
//! Seek semantics match mpv's hr-seek and FFmpeg's accurate seek: after
//! [`VideoReader::seek`] to `target`, the next frame returned is the first frame
//! whose presentation timestamp is `>= target`. Callers put the half-frame bias
//! into `target` (see the time map), so that rounding in timestamps never selects
//! the neighboring frame.

use std::path::Path;

use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, codec, filter, format, frame, media, software::scaling};

/// An audio frame that starts later than the frame before it ended, by more
/// than this (s), follows a gap: a damaged packet, or one the file lacks.
/// Smaller differences are timestamp rounding (Matroska keeps ms).
pub(crate) const AUDIO_GAP: f64 = 0.002;

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
    let mut drain = |decoder: &mut ffmpeg::decoder::Opened, frame: &mut F| loop {
        match decoder.receive_frame(frame) {
            Ok(()) => each(frame)?,
            Err(ffmpeg::Error::InvalidData) => {}
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

/// Decodes the primary video stream of a file in presentation order.
pub struct VideoReader {
    path: std::path::PathBuf,
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
        let mut reader = Self::open_raw(path)?;
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

    fn open_raw(path: &Path) -> Result<Self> {
        crate::init();
        let input = format::input(path).ctx(|| format!("opening {}", path.display()))?;
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
            path: path.to_path_buf(),
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
    pub fn path(&self) -> &Path {
        &self.path
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
        let fresh = Self::open_raw(&self.path)?;
        self.input = fresh.input;
        self.decoder = fresh.decoder;
        self.draining = false;
        Ok(())
    }

    /// Decodes the next frame from the stream, feeding packets as needed.
    fn decode_one(&mut self) -> Result<Option<frame::Video>> {
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
                Err(ffmpeg::Error::InvalidData) => continue,
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
