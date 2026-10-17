use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, codec, filter, format, frame, media, software::scaling};

pub(crate) mod standalone;

pub(crate) const AUDIO_GAP: f64 = 0.002;

pub(crate) const AUDIO_GAP_BOUND: f64 = 1.0;

pub(crate) const DAMAGED_IN_A_ROW: u32 = 100;

pub(crate) fn file_end(
    start: Option<f64>,
    duration: Option<f64>,
    origin: Option<f64>,
) -> Option<f64> {
    let duration = duration.filter(|d| *d > 0.0)?;
    Some(start.or(origin)? + duration)
}

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

pub(crate) fn stream_origin(start_pts: i64, tick: f64) -> Option<f64> {
    (start_pts != ffmpeg::ffi::AV_NOPTS_VALUE).then_some(start_pts as f64 * tick)
}

fn opt_av_time(ts: i64) -> Option<f64> {
    (ts != ffmpeg::ffi::AV_NOPTS_VALUE).then_some(ts as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE))
}

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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Input {
    File(PathBuf),
    Sequence {
        pattern: PathBuf,
        first: u64,
        rate: (i32, i32),
    },
}

impl Input {
    pub fn path(&self) -> &Path {
        match self {
            Input::File(p) => p,
            Input::Sequence { pattern, .. } => pattern,
        }
    }

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

static DECODED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn frames_decoded() -> u64 {
    DECODED.load(std::sync::atomic::Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    Stream,
    Frames,
}

pub struct VideoReader {
    source: Input,
    reading: Reading,
    input: format::context::Input,
    stream: usize,
    time_base: ffmpeg::Rational,
    start_pts: Option<i64>,
    decoder: ffmpeg::decoder::Video,
    draining: bool,
    min_pts: Option<i64>,
    pending: Option<frame::Video>,
    packet: Option<ffmpeg::Packet>,
    first_pts: Option<i64>,
    rotation: u16,
    first_props: Option<FrameProps>,
    deinterlacer: Option<Deinterlacer>,
    interlaced: bool,
    gpu: Option<crate::hw::GpuDecoder>,
    kind: Option<crate::hw::Kind>,
    decoding: Arc<Decoding>,
    standalone: Option<standalone::Standalone>,
    _open: Opened,
}

static OPEN: AtomicUsize = AtomicUsize::new(0);

pub fn open_readers() -> usize {
    OPEN.load(Ordering::Relaxed)
}

struct Opened(PathBuf);

impl Opened {
    fn new(path: PathBuf) -> Self {
        OPEN.fetch_add(1, Ordering::Relaxed);
        Self(path)
    }
}

impl Drop for Opened {
    fn drop(&mut self) {
        let left = OPEN.fetch_sub(1, Ordering::Relaxed) - 1;
        crate::debug!(
            "decoding",
            "closed on {}: {} ({left} readers open)",
            thread_name(),
            self.0.display()
        );
    }
}

fn thread_name() -> String {
    std::thread::current()
        .name()
        .unwrap_or("a thread")
        .to_string()
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Decoding {
    pub codec: String,
    pub profile: Option<String>,
    pub fps: Option<f64>,
    pub interlaced: bool,
    pub rotation: u16,
    pub decoder: String,
    pub gpu: Option<String>,
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

fn c_name(p: *const std::ffi::c_char) -> Option<String> {
    // SAFETY: FFmpeg's static names, or null.
    (!p.is_null()).then(|| {
        unsafe { std::ffi::CStr::from_ptr(p) }
            .to_string_lossy()
            .into_owned()
    })
}

pub(crate) fn deinterlacer(pixel: format::Pixel) -> &'static str {
    match pixel {
        format::Pixel::CUDA => "bwdif_cuda",
        format::Pixel::VULKAN => "bwdif_vulkan",
        _ => "bwdif",
    }
}

pub(crate) fn deinterlacing(
    f: &frame::Video,
    time_base: ffmpeg::Rational,
    reading: Reading,
) -> Result<crate::hw::GpuGraph> {
    let sar = f.aspect_ratio();
    let mut graph = crate::hw::GpuGraph::for_reading(reading);
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

struct Deinterlacer {
    graph: Option<crate::hw::GpuGraph>,
    time_base: ffmpeg::Rational,
    reading: Reading,
    input: (u32, u32, format::Pixel),
    gpu_failed: bool,
    flushed: bool,
    done: bool,
}

impl Deinterlacer {
    fn new(time_base: ffmpeg::Rational, reading: Reading) -> Self {
        Self {
            graph: None,
            time_base,
            reading,
            input: (0, 0, format::Pixel::None),
            gpu_failed: false,
            flushed: false,
            done: false,
        }
    }

    fn takes(&self, f: &frame::Video) -> bool {
        match f.format() {
            format::Pixel::CUDA | format::Pixel::VULKAN => {
                !self.gpu_failed && crate::hw::deinterlace::gives_bwdifs(f)
            }
            _ => !crate::hw::on_gpu(f),
        }
    }

    fn build(&mut self, f: &frame::Video) -> Result<()> {
        self.graph = Some(deinterlacing(f, self.time_base, self.reading)?);
        self.input = (f.width(), f.height(), f.format());
        Ok(())
    }

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

    fn pull(&mut self) -> Result<Option<frame::Video>> {
        let Some(g) = self.graph.as_mut() else {
            return Ok(None);
        };
        let mut sink = g.get("out").expect("sink");
        let mut out = frame::Video::empty();
        match sink.sink().frame(&mut out) {
            Ok(()) => {
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

pub(crate) fn gpu_decoder(id: codec::Id, kind: crate::hw::Kind) -> Option<ffmpeg::Codec> {
    use ffmpeg::ffi;
    let id: ffi::AVCodecID = id.into();
    let mut opaque = std::ptr::null_mut();
    // SAFETY: FFmpeg's codecs and their hardware configs are static; both lists end with null.
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

unsafe extern "C" fn gpu_format(
    ctx: *mut ffmpeg::ffi::AVCodecContext,
    formats: *const ffmpeg::ffi::AVPixelFormat,
) -> ffmpeg::ffi::AVPixelFormat {
    // SAFETY: FFmpeg's context, its device set before opening; `formats` ends with AV_PIX_FMT_NONE.
    unsafe {
        let wanted = crate::hw::Kind::of_device((*ctx).hw_device_ctx).map(|k| k.pixel().into());
        let mut p = formats;
        while *p != ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_NONE {
            if Some(*p) == wanted {
                #[cfg(target_os = "linux")]
                if *p == ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_VAAPI {
                    crate::hw::vaapi::share_frames(ctx);
                }
                return *p;
            }
            p = p.add(1);
        }
        ffmpeg::ffi::avcodec_default_get_format(ctx, formats)
    }
}

pub fn h264_open_gop(source: &Input) -> bool {
    remembered(source, |s| keyframes_not_idr(s).ok().flatten())
}

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

fn keyframes_not_idr(source: &Input) -> Result<Option<bool>> {
    let mut input = source.open()?;
    let Some(stream) = input.streams().best(media::Type::Video) else {
        return Ok(None);
    };
    let index = stream.index();
    // SAFETY: the stream's parameters, read only.
    let lengths = unsafe {
        let p = &*stream.parameters().as_ptr();
        (p.extradata_size > 4 && *p.extradata == 1)
            .then(|| usize::from(*p.extradata.add(4) & 3) + 1)
    };
    let open = |p: &ffmpeg::Packet| not_idr(p.data().unwrap_or_default(), lengths);
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
            // SAFETY: the opened context; FFmpeg keeps a byte position within the file.
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

enum Next {
    Key,
    End,
    Far,
}

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

pub(crate) fn gpu_takes(
    id: codec::Id,
    gpu: &crate::hw::GpuDecoder,
    open_gop: impl FnOnce() -> bool,
) -> bool {
    on_gpu(id, gpu, open_gop).is_ok()
}

pub(crate) fn open_video(
    mut ctx: codec::context::Context,
    id: codec::Id,
    gpu: Option<&crate::hw::GpuDecoder>,
    open_gop: impl FnOnce() -> bool,
) -> Result<ffmpeg::decoder::Video> {
    crate::buffers::own(&mut ctx);
    let Some((_, device, codec)) = gpu.and_then(|g| on_gpu(id, g, open_gop).ok()) else {
        return ctx.decoder().video().ctx(|| "opening video decoder".into());
    };
    // SAFETY: the context isn't opened yet; it takes a reference of its own.
    unsafe {
        let raw = ctx.as_mut_ptr();
        let hw = ffmpeg::ffi::av_buffer_ref(device.as_ptr());
        if hw.is_null() {
            return Err(Error::Invalid("no memory for a GPU device".into()));
        }
        (*raw).hw_device_ctx = hw;
        (*raw).get_format = Some(gpu_format);
        (*raw).extra_hw_frames = 8;
        (*raw).thread_count = 1;
    }
    ctx.decoder()
        .open_as(codec)
        .and_then(|d| d.video())
        .ctx(|| "opening video decoder".into())
}

pub(crate) fn in_memory(f: frame::Video) -> Result<frame::Video> {
    if crate::hw::on_gpu(&f) {
        crate::hw::download(&f)
    } else {
        Ok(f)
    }
}

pub(crate) fn decoded_on(f: &frame::Video, gpu: Option<&crate::hw::GpuDecoder>) -> String {
    match (gpu, crate::hw::Kind::of_frame(f)) {
        (Some(gpu), Some(kind)) => gpu.labeled(kind),
        _ => "the CPU".into(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameProps {
    pub top_field_first: Option<bool>,
    pub transfer: ffmpeg::ffi::AVColorTransferCharacteristic,
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
        Self::open_as(source, None, Reading::Stream)
    }

    pub fn open_as(
        source: &Input,
        gpu: Option<&crate::hw::GpuDecoder>,
        reading: Reading,
    ) -> Result<Self> {
        if let Some(gpu) = gpu {
            return Self::open_gpu_or_cpu_as(source, gpu, reading);
        }
        let mut reader = Self::open_with(source, None, reading)?;
        reader.opened(None);
        Ok(reader)
    }

    pub fn open_on(source: &Input, gpu: &crate::hw::GpuDecoder) -> Result<Self> {
        Self::open_on_as(source, gpu, Reading::Stream)
    }

    fn open_on_as(source: &Input, gpu: &crate::hw::GpuDecoder, reading: Reading) -> Result<Self> {
        let mut reader = Self::open_with(source, Some(gpu), reading)?;
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

    pub fn open_gpu_or_cpu(source: &Input, gpu: &crate::hw::GpuDecoder) -> Result<Self> {
        Self::open_gpu_or_cpu_as(source, gpu, Reading::Stream)
    }

    fn open_gpu_or_cpu_as(
        source: &Input,
        gpu: &crate::hw::GpuDecoder,
        reading: Reading,
    ) -> Result<Self> {
        Self::open_on_as(source, gpu, reading).or_else(|e| {
            let mut reader = Self::open_with(source, None, reading)?;
            reader.opened(Some(&format!("not on {gpu}: {e}")));
            Ok(reader)
        })
    }

    pub fn decoding(&self) -> &Arc<Decoding> {
        &self.decoding
    }

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
            "{}, {}: {}; first frame {}; {}",
            thread_name(),
            self.path().display(),
            self.decoding,
            self.first_frame(),
            self.open_note()
        );
    }

    fn open_note(&self) -> String {
        // SAFETY: the opened decoder's context, read only.
        let threads = unsafe { (*self.decoder.as_ptr()).thread_count };
        let threads = match threads {
            _ if self.standalone.is_some() => standalone::Standalone::note(),
            1 => "1 thread".to_string(),
            n => format!("{n} threads"),
        };
        format!("{threads}, {} readers open", open_readers())
    }

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

    fn open_with(
        source: &Input,
        device: Option<&crate::hw::GpuDecoder>,
        reading: Reading,
    ) -> Result<Self> {
        let mut reader = Self::open_raw(source, device, reading)?;
        reader.pending = reader.decode_one()?;
        reader.first_pts = reader.pending.as_ref().and_then(|f| f.pts());
        reader.first_props = reader.pending.as_ref().map(FrameProps::of);
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
            reader.deinterlacer = Some(Deinterlacer::new(reader.time_base, reading));
        }
        Ok(reader)
    }

    fn open_raw(
        source: &Input,
        device: Option<&crate::hw::GpuDecoder>,
        reading: Reading,
    ) -> Result<Self> {
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
        unsafe { (*ctx.as_mut_ptr()).pkt_timebase = time_base.into() };
        crate::buffers::own(&mut ctx);
        let codec_id = stream.parameters().id();
        let on = match device {
            Some(d) => Some(on_gpu(codec_id, d, || h264_open_gop(source))?),
            None => None,
        };
        match &on {
            Some((_, device, _)) => unsafe {
                let raw = ctx.as_mut_ptr();
                let hw = ffmpeg::ffi::av_buffer_ref(device.as_ptr());
                if hw.is_null() {
                    return Err(Error::Invalid("no memory for a GPU device".into()));
                }
                (*raw).hw_device_ctx = hw;
                (*raw).get_format = Some(gpu_format);
                (*raw).extra_hw_frames = 8;
                (*raw).thread_count = 1;
            },
            None => ctx.set_threading(codec::threading::Config {
                kind: codec::threading::Type::Frame,
                count: crate::pool::threads_for(reading),
            }),
        }
        let standalone = match on {
            None => standalone::Standalone::for_stream(source, &stream, reading),
            Some(_) => None,
        };
        if standalone.is_some() {
            ctx.set_threading(standalone::one_thread());
        }
        let decoder = match &on {
            Some((_, _, codec)) => ctx.decoder().open_as(*codec).and_then(|d| d.video()),
            None => ctx.decoder().video(),
        };
        let decoder = crate::parts::decoder_opened(decoder, codec_id, "video")?;
        let open = Opened::new(path.to_path_buf());
        crate::export::audio::discard_others(&input, index);
        Ok(Self {
            source: source.clone(),
            reading,
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
            standalone,
            _open: open,
        })
    }

    pub fn gpu(&self) -> Option<crate::hw::Kind> {
        self.kind
    }

    pub(crate) fn decoders_cuda(&self) -> Option<&crate::cuda::CudaDevice> {
        self.gpu
            .as_ref()
            .filter(|_| self.kind.is_some())
            .and_then(crate::hw::GpuDecoder::cuda)
    }

    pub fn interlaced(&self) -> bool {
        self.interlaced
    }

    pub fn peek(&self) -> Option<&frame::Video> {
        self.pending.as_ref()
    }

    pub(crate) fn frames_stay(&self) -> bool {
        match (&self.deinterlacer, &self.pending) {
            (Some(d), Some(first)) => d.takes(first),
            _ => true,
        }
    }

    pub fn time_base(&self) -> ffmpeg::Rational {
        self.time_base
    }

    pub fn path(&self) -> &Path {
        self.source.path()
    }

    pub fn input(&self) -> &Input {
        &self.source
    }

    pub fn rotation(&self) -> u16 {
        self.rotation
    }

    pub fn frame_rate(&self) -> Option<ffmpeg::Rational> {
        let r = self.input.stream(self.stream)?.avg_frame_rate();
        (r.numerator() > 0 && r.denominator() > 0).then_some(r)
    }

    pub fn first_props(&self) -> Option<FrameProps> {
        self.first_props
    }

    pub fn start_pts(&self) -> Option<i64> {
        self.start_pts
    }

    pub fn first_pts(&self) -> Option<i64> {
        self.first_pts
    }

    pub fn pending_pts(&self) -> Option<i64> {
        self.pending.as_ref().and_then(|f| f.pts())
    }

    pub fn passes_before(&self) -> Option<i64> {
        self.min_pts
    }

    pub fn from_keyframe(&mut self) {
        if let Some(pts) = self.pending_pts() {
            self.min_pts = Some(pts);
        }
    }

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

    pub fn seek(&mut self, target: i64) -> Result<()> {
        if self.deinterlacer.is_some() {
            self.deinterlacer = Some(Deinterlacer::new(self.time_base, self.reading));
        }
        self.pending = None;
        self.min_pts = None;
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
                self.flush();
            }
            self.min_pts = None;
            self.pending = None;
            let frame = self.decode_one()?;
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

    fn rewind(&mut self) -> Result<()> {
        let input = self.source.open()?;
        crate::export::audio::discard_others(&input, self.stream);
        self.input = input;
        self.flush();
        Ok(())
    }

    fn flush(&mut self) {
        self.decoder.flush();
        self.draining = false;
        if let Some(s) = self.standalone.as_mut() {
            s.flushed();
        }
    }

    fn decode_one(&mut self) -> Result<Option<frame::Video>> {
        if self.standalone.is_some() {
            return standalone::decode_one(self);
        }
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

pub fn read_barcode(frame: &frame::Video) -> Result<Option<u32>> {
    use format::Pixel;
    let (w, h) = (frame.width() as usize, frame.height() as usize);
    match frame.format() {
        Pixel::YUV420P
        | Pixel::YUV422P
        | Pixel::YUV444P
        | Pixel::YUVJ420P
        | Pixel::YUVJ422P
        | Pixel::YUVJ444P
        | Pixel::NV12
        | Pixel::GRAY8 => Ok(crate::barcode::read(frame.data(0), w, h, frame.stride(0))),
        other => {
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
        assert!(not_idr(&[0, 0, 0, 2, 0x41, 0x9a], Some(4)));
    }

    #[test]
    fn an_idr_slice_makes_it_idr() {
        assert!(!not_idr(&[0, 0, 0, 2, 0x65, 0x88], Some(4)));
    }

    #[test]
    fn start_codes_lead_to_the_nal_units() {
        assert!(!not_idr(
            &[0, 0, 0, 1, 0x09, 0xf0, 0, 0, 1, 0x65, 0x88],
            None
        ));
    }
}
