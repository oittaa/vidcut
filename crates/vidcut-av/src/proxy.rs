//! Playback proxies: a small, easy-to-decode copy of a source's pictures,
//! AV1, H.264, ProRes Proxy or DNxHR LB (decision D41).
//!
//! Each proxy frame keeps the source frame's presentation timestamp, in the
//! source time base when that base is ordinary and in microseconds otherwise.
//! The preview can then swap the file and keep the same EDL `start` math
//! (decision D41). Audio is not copied: preview audio stays on the original.
//!
//! The file is Matroska, written to a temporary name and renamed into place.
//! Pictures are turned upright and scaled to `width`×`height` as 8-bit 4:2:0
//! (ProRes 10-bit 4:2:2, DNxHR 8-bit 4:2:2). Every encoder makes
//! a keyframe every 60 frames (ProRes and DNxHR every frame) and shows frames
//! in the order it codes them: a seek decodes at most 60 frames, and the
//! preview needs no tail patches for a proxy.

use std::collections::HashMap;
use std::path::Path;

use vidcut_core::export::EncoderChoice;
use vidcut_core::preferences::ProxyCodec;

use crate::decode::{Input, VideoReader};
use crate::error::{Context, Error, Result};
use crate::export::encode::{Availability, Upload};
use crate::export::video::Normalizer;
use crate::ffmpeg::{self, codec, encoder, ffi, format, frame};

/// Frames between keyframes.
const KEYFRAMES: u32 = 60;

/// An encoder proxies can be made with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyEncoder {
    /// FFmpeg's encoder: `av1_nvenc`, `av1_vaapi`, `h264_nvenc`,
    /// `h264_amf`, `h264_vaapi`, `h264_qsv`, `h264_videotoolbox`,
    /// `libsvtav1`, `libx264` (where this FFmpeg has neither of those two
    /// `libopenh264` or `prores_ks`), `prores_videotoolbox`,
    /// `prores_ks_vulkan`, `prores_ks` or `dnxhd`.
    pub name: String,
    /// `av1`, `h264`, `prores` or `dnxhr`.
    pub codec: &'static str,
    /// Hardware (NVENC, AMF, VAAPI, QSV, VideoToolbox), the GPU through
    /// Vulkan, or software (CPU).
    pub kind: EncoderChoice,
    /// The GPU it runs on, as the encoder check names it: a VAAPI render
    /// node, an AMF or QSV adapter or node, Vulkan's GPU by its id.
    pub device: Option<String>,
}

impl ProxyEncoder {
    fn software(name: &str, codec: &'static str) -> Self {
        Self {
            name: name.into(),
            codec,
            kind: EncoderChoice::Cpu,
            device: None,
        }
    }

    /// What it is, for people: `AV1, NVIDIA hardware (NVENC)`.
    pub fn label(&self) -> String {
        let codec = match self.codec {
            "av1" => "AV1",
            "prores" => "ProRes Proxy",
            "dnxhr" => "DNxHR LB",
            _ => "H.264",
        };
        let what = match (self.kind, self.name.as_str()) {
            (EncoderChoice::Nvenc, _) => "NVIDIA hardware (NVENC)".to_string(),
            (EncoderChoice::Amf, _) => "AMD hardware (AMF)".to_string(),
            (EncoderChoice::Qsv, _) => "Intel hardware (QSV)".to_string(),
            (EncoderChoice::Vaapi, _) => format!(
                "VAAPI hardware ({})",
                self.device.as_deref().unwrap_or("GPU")
            ),
            (EncoderChoice::VideoToolbox, _) => "Apple hardware (VideoToolbox)".to_string(),
            (EncoderChoice::Vulkan, _) => "GPU (Vulkan)".to_string(),
            (_, "libsvtav1") => "software (SVT-AV1)".to_string(),
            (_, "libx264") => "software (x264)".to_string(),
            (_, "libopenh264") => "software (OpenH264)".to_string(),
            (_, "prores_ks" | "dnxhd") => "software (FFmpeg's)".to_string(),
            (_, other) => format!("software ({other})"),
        };
        format!("{codec}, {what}")
    }

    fn pixel(&self) -> format::Pixel {
        match (self.kind, self.codec) {
            (
                EncoderChoice::Amf
                | EncoderChoice::Qsv
                | EncoderChoice::Vaapi
                | EncoderChoice::VideoToolbox,
                _,
            ) => format::Pixel::NV12,
            (_, "prores") => format::Pixel::YUV422P10LE,
            (_, "dnxhr") => format::Pixel::YUV422P,
            _ => format::Pixel::YUV420P,
        }
    }

    /// Its settings: quality like the x264 proxies' (`crf 20`), a keyframe
    /// every [`KEYFRAMES`] frames, and nothing coded ahead of where it shows.
    fn options(&self) -> Vec<(&'static str, String)> {
        let keys = KEYFRAMES.to_string();
        match self.name.as_str() {
            "libx264" => vec![
                ("preset", "veryfast".into()),
                ("crf", "20".into()),
                (
                    "x264-params",
                    format!("bframes=0:keyint={keys}:min-keyint={keys}:scenecut=0"),
                ),
            ],
            // The keyframe interval comes from the context's GOP.
            "libsvtav1" => vec![
                ("preset", "10".into()),
                ("crf", "32".into()),
                ("svtav1-params", "scd=0:pred-struct=1".into()),
            ],
            "av1_nvenc" | "h264_nvenc" => vec![
                ("preset", "p4".into()),
                ("tune", "hq".into()),
                ("rc", "vbr".into()),
                ("cq", if self.codec == "av1" { "32" } else { "20" }.into()),
                ("b", "0".into()),
            ],
            // One quantizer for every frame, as x264's CRF 20.
            "h264_amf" => vec![
                ("rc", "cqp".into()),
                ("qp_i", "20".into()),
                ("qp_p", "20".into()),
            ],
            "h264_vaapi" => vec![("rc_mode", "CQP".into()), ("qp", "20".into())],
            // Its intelligent constant quality.
            "h264_qsv" => vec![("global_quality", "20".into())],
            // Constant quality on its scale of 0 to 100, the higher the
            // better: 67 is the exports' for x264's CRF 20.
            "h264_videotoolbox" => vec![
                ("flags", "+qscale".into()),
                ("global_quality", (67 * ffi::FF_QP2LAMBDA).to_string()),
            ],
            // One quantizer for every frame, near x264's CRF 20.
            "libopenh264" => vec![
                ("profile", "high".into()),
                ("rc_mode", "quality".into()),
                ("qmin", "22".into()),
                ("qmax", "22".into()),
            ],
            // ProRes Proxy: every frame a keyframe.
            "prores_ks" | "prores_ks_vulkan" => {
                vec![("profile", "0".into()), ("vendor", "apl0".into())]
            }
            // Apple's ProRes hardware only: without it the session won't
            // open, and FFmpeg's own takes over.
            "prores_videotoolbox" => vec![("profile", "proxy".into()), ("allow_sw", "0".into())],
            // DNxHR LB, 8-bit 4:2:2: every frame a keyframe.
            "dnxhd" => vec![("profile", "dnxhr_lb".into())],
            // AV1's quantizer index (0–255) is its global quality: it has
            // no `qp`.
            "av1_vaapi" => vec![("rc_mode", "CQP".into()), ("global_quality", "128".into())],
            _ => Vec::new(),
        }
    }
}

/// The codecs automatic chooses among, in its order: those that make every
/// frame a keyframe first.
pub const AUTOMATIC: [ProxyCodec; 4] = [
    ProxyCodec::Dnxhr,
    ProxyCodec::Prores,
    ProxyCodec::Av1,
    ProxyCodec::H264,
];

/// The encoders `choice` makes proxies with here, best first, where the GPU
/// in use plays the codecs `plays` names (`av1`, `h264`, `prores`).
///
/// A codec's own: the hardware ones this machine has working (AV1's NVENC,
/// then VAAPI; H.264's NVENC, AMF, VAAPI, QSV, VideoToolbox; ProRes's
/// VideoToolbox, then the GPU's through Vulkan), then the first software
/// one this FFmpeg has: SVT-AV1 for AV1 and x264 for H.264 (without those,
/// OpenH264, else ProRes Proxy), FFmpeg's own for ProRes and DNxHR.
///
/// Automatic takes every codec's ([`AUTOMATIC`]) in four rounds, each in
/// that order: those the GPU's video engine makes and the GPU plays, those
/// it plays, those the GPU makes (through Vulkan too), the rest. A proxy
/// takes the first that opens: NVENC allows a few sessions at a time, and
/// DNxHR's are at least 256×120.
pub fn encoders(choice: ProxyCodec, available: &Availability, plays: &[&str]) -> Vec<ProxyEncoder> {
    if choice != ProxyCodec::Auto {
        return own(choice, available);
    }
    let mut all: Vec<ProxyEncoder> = Vec::new();
    for codec in AUTOMATIC {
        for e in own(codec, available) {
            if !all.iter().any(|a| a.name == e.name) {
                all.push(e);
            }
        }
    }
    all.sort_by_key(|e| round(e, plays));
    all
}

/// Automatic's round for `e`, the first 0: made by the GPU's video engine
/// and played on the GPU; played on it (made in software or through
/// Vulkan); made on it; neither.
fn round(e: &ProxyEncoder, plays: &[&str]) -> u8 {
    let played = plays.contains(&e.codec);
    match e.kind {
        EncoderChoice::Cpu | EncoderChoice::Vulkan if played => 1,
        EncoderChoice::Cpu => 3,
        _ if played => 0,
        _ => 2,
    }
}

/// `codec`'s own encoders here, best first ([`encoders`]).
fn own(codec: ProxyCodec, available: &Availability) -> Vec<ProxyEncoder> {
    use EncoderChoice::{Amf, Nvenc, Qsv, Vaapi, VideoToolbox, Vulkan};
    use format::Pixel::{NV12, YUV420P, YUV422P10LE};
    let hardware: &[(&str, &'static str, EncoderChoice, format::Pixel)] = match codec {
        ProxyCodec::Av1 => &[
            ("av1_nvenc", "av1", Nvenc, YUV420P),
            ("av1_vaapi", "av1", Vaapi, NV12),
        ],
        ProxyCodec::H264 => &[
            ("h264_nvenc", "h264", Nvenc, YUV420P),
            ("h264_amf", "h264", Amf, NV12),
            ("h264_vaapi", "h264", Vaapi, NV12),
            ("h264_qsv", "h264", Qsv, NV12),
            ("h264_videotoolbox", "h264", VideoToolbox, NV12),
        ],
        ProxyCodec::Prores => &[
            ("prores_videotoolbox", "prores", VideoToolbox, NV12),
            ("prores_ks_vulkan", "prores", Vulkan, YUV422P10LE),
        ],
        ProxyCodec::Dnxhr | ProxyCodec::Auto => &[],
    };
    let mut out = Vec::new();
    for &(name, codec, kind, pixel) in hardware {
        if let Some(status) = available.find(name, pixel) {
            out.push(ProxyEncoder {
                name: name.into(),
                codec,
                kind,
                device: status.device.clone(),
            });
        }
    }
    // The first software encoder this FFmpeg has. ProRes and DNxHR are
    // FFmpeg's own, in every build.
    let software: &[(&str, &'static str)] = match codec {
        ProxyCodec::Av1 => &[
            ("libsvtav1", "av1"),
            ("libx264", "h264"),
            ("libopenh264", "h264"),
            ("prores_ks", "prores"),
        ],
        ProxyCodec::H264 => &[
            ("libx264", "h264"),
            ("libopenh264", "h264"),
            ("prores_ks", "prores"),
        ],
        ProxyCodec::Prores => &[("prores_ks", "prores")],
        ProxyCodec::Dnxhr => &[("dnxhd", "dnxhr")],
        ProxyCodec::Auto => &[],
    };
    if let Some((name, codec)) = software.iter().find(|(n, _)| crate::parts::has_encoder(n)) {
        out.push(ProxyEncoder::software(name, codec));
    }
    out
}

/// What [`write`] produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenProxy {
    pub path: std::path::PathBuf,
    pub width: u32,
    pub height: u32,
    /// Frames written.
    pub frames: u64,
    /// The encoder it was made with.
    pub encoder: ProxyEncoder,
    /// Decoded and scaled on the GPU (NVDEC and CUDA), frames never leaving
    /// it, rather than on the CPU.
    pub on_gpu: bool,
    /// The GPU that decoded it (frames copied down unless `on_gpu`); `None`:
    /// the CPU.
    pub decoder: Option<crate::hw::Kind>,
}

/// Encodes `input`'s pictures to `dest` at `width`×`height` (even), with the
/// first of `encoders` that opens. Each frame is reported to this thread's
/// [`crate::progress`] guard, out of `total` (the source's frame count, when
/// known); this thread's [`crate::cancel`] flag stops it, leaving no file.
///
/// The frames decode on `decoder`, a GPU's, where it decodes the stream,
/// on the CPU otherwise. NVENC takes NVDEC's frames deinterlaced, turned,
/// converted and scaled on the GPU, never copied to memory
/// ([`CudaPictures`](crate::cuda::CudaPictures)). For every other encoder
/// the CPU scales, the GPU's frames shrunk there before they come down
/// (`hw::Shrinker`).
pub fn write(
    input: &Input,
    width: u32,
    height: u32,
    dest: &Path,
    total: Option<u64>,
    encoders: &[ProxyEncoder],
    decoder: Option<&crate::hw::GpuDecoder>,
) -> Result<WrittenProxy> {
    crate::init();
    if width < 2 || height < 2 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(Error::Invalid(format!(
            "proxy size {width}x{height} must be even and at least 2"
        )));
    }
    if encoders.is_empty() {
        return Err(Error::Invalid("no encoder to make the proxy with".into()));
    }
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Invalid(format!("creating {}: {e}", parent.display())))?;
    }
    let tmp = dest.with_extension("part.mkv");
    let shown = Shown {
        dest,
        total: total.map_or(0, |t| t as i64),
        started: std::time::Instant::now(),
    };
    let done = |frames, enc: &ProxyEncoder, on_gpu, decoder| -> Result<WrittenProxy> {
        std::fs::rename(&tmp, dest)
            .map_err(|e| Error::Invalid(format!("replacing {}: {e}", dest.display())))?;
        Ok(WrittenProxy {
            path: dest.to_path_buf(),
            width,
            height,
            frames,
            encoder: enc.clone(),
            on_gpu,
            decoder,
        })
    };
    let mut errors = Vec::new();
    for enc in encoders {
        // A fresh reader: a failed attempt may have read frames already.
        let mut reader = match decoder {
            Some(d) => VideoReader::open_gpu_or_cpu(input, d)?,
            None => VideoReader::open_input(input)?,
        };
        match encode_into(&mut reader, enc, (width, height), &tmp, &shown) {
            Ok((frames, on_gpu)) => return done(frames, enc, on_gpu, reader.gpu()),
            Err(Failed::Encoding(e)) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
            // An encoder that won't open makes way for the next.
            Err(Failed::Opening(e)) => {
                let _ = std::fs::remove_file(&tmp);
                errors.push(e);
            }
        }
    }
    Err(errors.remove(0))
}

/// The encoder's time base: the source's, when its denominator is a sane
/// clock, otherwise microseconds. Either way a frame's timestamp in seconds
/// matches the source.
fn encode_time_base(src: ffmpeg::Rational) -> ffmpeg::Rational {
    let (n, d) = (src.numerator(), src.denominator());
    if n > 0 && (1..=1_000_000).contains(&d) {
        src
    } else {
        ffmpeg::Rational(1, 1_000_000)
    }
}

fn scale_pts(pts: i64, from: ffmpeg::Rational, to: ffmpeg::Rational) -> i64 {
    if from == to {
        return pts;
    }
    let n = i128::from(pts) * i128::from(from.numerator()) * i128::from(to.denominator());
    let d = i128::from(from.denominator().max(1)) * i128::from(to.numerator().max(1));
    (n / d) as i64
}

/// Where progress goes: the file it is for, out of how many frames, since
/// when.
struct Shown<'a> {
    dest: &'a Path,
    total: i64,
    started: std::time::Instant,
}

impl Shown<'_> {
    fn frame(&self, frames: u64) {
        crate::progress::report(
            self.dest,
            crate::export::Progress::single(
                frames as i64,
                self.total.max(frames as i64),
                self.started.elapsed(),
            ),
        );
    }
}

/// Why an attempt failed: the encoder wouldn't open (another may), the
/// source isn't for the GPU (the CPU may do it), or encoding failed.
enum Failed {
    Opening(Error),
    Encoding(Error),
}

/// Opens `enc` for `width`×`height` frames timed in `enc_tb`: for VAAPI
/// and Vulkan, with the context that uploads them; with `gpu_frames`, for
/// frames in GPU memory of that frames context.
fn open(
    enc: &ProxyEncoder,
    (width, height): (u32, u32),
    enc_tb: ffmpeg::Rational,
    rate: Option<ffmpeg::Rational>,
    global: bool,
    gpu_frames: Option<*mut ffi::AVBufferRef>,
) -> Result<(encoder::Video, ffmpeg::Codec, Option<Upload>)> {
    // FFmpeg's DNxHR takes nothing smaller ("Input dimensions too small").
    if enc.name == "dnxhd" && (width < 256 || height < 120) {
        return Err(Error::Invalid(format!(
            "DNxHR proxies are at least 256×120 pixels, and this one would be \
             {width}×{height}: make the proxy size larger or choose another codec"
        )));
    }
    let codec = encoder::find_by_name(&enc.name)
        .ok_or_else(|| Error::Invalid(format!("this FFmpeg has no {} encoder", enc.name)))?;
    let mut ctx = codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()
        .ctx(|| format!("creating {}", enc.name))?;
    ctx.set_width(width);
    ctx.set_height(height);
    ctx.set_time_base(enc_tb);
    if let Some(r) = rate {
        ctx.set_frame_rate(Some(r));
    }
    ctx.set_gop(KEYFRAMES);
    // Nothing coded ahead of where it shows.
    ctx.set_max_b_frames(0);
    // FFmpeg's own ProRes and DNxHR encoders use one core unless told
    // otherwise.
    if matches!(enc.name.as_str(), "prores_ks" | "dnxhd") {
        ctx.set_threading(codec::threading::Config {
            kind: codec::threading::Type::Frame,
            count: 0, // let FFmpeg choose
        });
    }
    // Closed GOPs: every keyframe decodes on its own, so a seek can start
    // there (SVT-AV1 otherwise makes forward keyframes nothing seeks to).
    let mut flags = codec::Flags::CLOSED_GOP;
    if global {
        flags |= codec::Flags::GLOBAL_HEADER;
    }
    ctx.set_flags(flags);
    crate::color::tag(unsafe { ctx.as_mut_ptr() }, None);
    let hw_frames = |ctx: &mut encoder::video::Video, frames: *mut ffi::AVBufferRef| {
        let frames = unsafe { ffi::av_buffer_ref(frames) };
        if frames.is_null() {
            return Err(Error::Invalid("no memory for the GPU frames".into()));
        }
        unsafe { (*ctx.as_mut_ptr()).hw_frames_ctx = frames };
        Ok(())
    };
    let upload = match (&enc.device, enc.kind, gpu_frames) {
        (_, EncoderChoice::Nvenc, Some(frames)) => {
            ctx.set_format(format::Pixel::CUDA);
            hw_frames(&mut ctx, frames)?;
            None
        }
        (Some(node), EncoderChoice::Vaapi, _) => {
            let v = Upload::vaapi(node, enc.pixel(), width, height, None)?;
            ctx.set_format(format::Pixel::VAAPI);
            hw_frames(&mut ctx, v.frames)?;
            Some(v)
        }
        (Some(gpu), EncoderChoice::Vulkan, _) => {
            let up = Upload::vulkan(gpu, enc.pixel(), width, height)?;
            ctx.set_format(format::Pixel::VULKAN);
            hw_frames(&mut ctx, up.frames)?;
            Some(up)
        }
        (Some(on), EncoderChoice::Amf | EncoderChoice::Qsv, _) => {
            crate::export::encode::on_device(&mut ctx, enc.kind, on)?;
            ctx.set_format(enc.pixel());
            None
        }
        _ => {
            ctx.set_format(enc.pixel());
            None
        }
    };
    let mut opts = ffmpeg::Dictionary::new();
    for (k, v) in enc.options() {
        opts.set(k, &v);
    }
    let opened = ctx
        .open_as_with(codec, opts)
        .ctx(|| format!("opening {}", enc.name))?;
    Ok((opened, codec, upload))
}

/// The proxy file being written: pictures go in in order, each one's
/// duration known once the next arrives.
struct Writer<'a> {
    octx: format::context::Output,
    encoder: encoder::Video,
    upload: Option<Upload>,
    src_tb: ffmpeg::Rational,
    enc_tb: ffmpeg::Rational,
    durations: HashMap<i64, i64>,
    pending: Option<(frame::Video, i64)>,
    last_dur: i64,
    frames: u64,
    shown: &'a Shown<'a>,
}

impl<'a> Writer<'a> {
    /// Creates `tmp` and opens `enc` for pictures timed in `src_tb` (with
    /// `gpu_frames`, pictures in GPU memory).
    fn new(
        tmp: &Path,
        enc: &ProxyEncoder,
        size: (u32, u32),
        (src_tb, rate): (ffmpeg::Rational, Option<ffmpeg::Rational>),
        gpu_frames: Option<*mut ffi::AVBufferRef>,
        shown: &'a Shown<'a>,
    ) -> std::result::Result<Self, Failed> {
        let enc_tb = encode_time_base(src_tb);
        let mut octx = format::output_as(tmp, "matroska")
            .ctx(|| format!("creating {}", tmp.display()))
            .map_err(Failed::Encoding)?;
        let global = octx.format().flags().contains(format::Flags::GLOBAL_HEADER);
        let (encoder, codec, upload) =
            open(enc, size, enc_tb, rate, global, gpu_frames).map_err(Failed::Opening)?;
        let mut header = || -> Result<()> {
            let mut ost = octx
                .add_stream(Some(codec))
                .ctx(|| "adding the proxy stream".into())?;
            ost.set_parameters(&encoder);
            ost.set_time_base(enc_tb);
            octx.write_header()
                .ctx(|| "writing the proxy header".into())?;
            Ok(())
        };
        header().map_err(Failed::Encoding)?;
        Ok(Self {
            octx,
            encoder,
            upload,
            src_tb,
            enc_tb,
            durations: HashMap::new(),
            pending: None,
            last_dur: 1,
            frames: 0,
            shown,
        })
    }

    /// The next picture, `pts` in the source's time base.
    fn push(&mut self, picture: frame::Video, pts: Option<i64>) -> Result<()> {
        let Some(raw) = pts else {
            return Err(Error::Invalid(
                "a source frame has no timestamp to copy onto the proxy".into(),
            ));
        };
        let mut pts = scale_pts(raw, self.src_tb, self.enc_tb);
        if let Some((_, prev)) = &self.pending
            && pts <= *prev
        {
            pts = prev.saturating_add(1);
        }
        if let Some((prev, prev_pts)) = self.pending.take() {
            let dur = (pts - prev_pts).max(1);
            self.last_dur = dur;
            self.encode(prev, prev_pts, dur)?;
        }
        self.pending = Some((picture, pts));
        Ok(())
    }

    fn encode(&mut self, mut picture: frame::Video, pts: i64, dur: i64) -> Result<()> {
        picture.set_pts(Some(pts));
        // The source's picture type would force it on the encoder.
        picture.set_kind(ffmpeg::picture::Type::None);
        unsafe { (*picture.as_mut_ptr()).duration = dur };
        let mut picture = match &self.upload {
            Some(up) => up.upload(&picture)?,
            None => picture,
        };
        picture.set_pts(Some(pts));
        self.durations.insert(pts, dur);
        self.encoder
            .send_frame(&picture)
            .ctx(|| "encoding a proxy frame".into())?;
        drain(
            &mut self.encoder,
            &mut self.octx,
            self.enc_tb,
            &mut self.durations,
        )?;
        self.frames += 1;
        self.shown.frame(self.frames);
        Ok(())
    }

    /// The last picture, then the end; returns the frames written.
    fn finish(mut self, source: &Path) -> Result<u64> {
        if let Some((prev, prev_pts)) = self.pending.take() {
            let dur = self.last_dur;
            self.encode(prev, prev_pts, dur)?;
        }
        if self.frames == 0 {
            return Err(Error::Invalid(format!(
                "{}: no pictures to proxy",
                source.display()
            )));
        }
        self.encoder
            .send_eof()
            .ctx(|| "flushing the proxy encoder".into())?;
        drain(
            &mut self.encoder,
            &mut self.octx,
            self.enc_tb,
            &mut self.durations,
        )?;
        self.octx
            .write_trailer()
            .ctx(|| "finishing the proxy".into())?;
        Ok(self.frames)
    }
}

/// Encodes what `reader` decodes: for NVENC, NVDEC's frames scaled on the
/// GPU and handed to it there; otherwise scaled with the export's
/// normalizer, on the CPU or shrunk on the GPU first. The frames written,
/// and whether they stayed on the GPU.
fn encode_into(
    reader: &mut VideoReader,
    enc: &ProxyEncoder,
    (width, height): (u32, u32),
    tmp: &Path,
    shown: &Shown<'_>,
) -> std::result::Result<(u64, bool), Failed> {
    let rotation = reader.rotation();
    let times = (reader.time_base(), reader.frame_rate());
    let size = (width, height);
    let mut pictures = reader
        .cuda()
        .filter(|_| enc.kind == EncoderChoice::Nvenc)
        .and_then(|d| {
            crate::cuda::CudaPictures::new(d, reader, size, format::Pixel::NV12, None, false).ok()
        });
    let gpu_frames = pictures
        .as_mut()
        .map(|p| p.frames_ctx())
        .filter(|f| !f.is_null());
    if gpu_frames.is_none() {
        pictures = None;
    }
    let mut writer = Writer::new(tmp, enc, size, times, gpu_frames, shown)?;
    let mut normalizer: Option<Normalizer> = None;
    let mut shrinker = None;
    let mut encoding = || -> Result<()> {
        loop {
            let src = reader.next_frame()?;
            // NVENC's go to the GPU's scaler, and to the encoder as it
            // makes them; nothing goes in at the end, for the last ones.
            if let Some(pictures) = pictures.as_mut() {
                pictures.push(src.as_ref())?;
                while let Some(p) = pictures.pull()? {
                    let pts = p.pts();
                    writer.push(p, pts)?;
                }
                if src.is_none() {
                    return Ok(());
                }
                continue;
            }
            let Some(src) = src else {
                return Ok(());
            };
            let src = if crate::hw::on_gpu(&src) {
                crate::export::video::down(&mut shrinker, &src, rotation, size)?
            } else {
                src
            };
            let picture = scale(&mut normalizer, &src, rotation, size, enc.pixel())?;
            writer.push(picture, src.pts())?;
        }
    };
    encoding().map_err(Failed::Encoding)?;
    let on_gpu = pictures.is_some();
    let frames = writer.finish(reader.path()).map_err(Failed::Encoding)?;
    Ok((frames, on_gpu))
}

fn scale(
    normalizer: &mut Option<Normalizer>,
    src: &frame::Video,
    rotation: u16,
    (width, height): (u32, u32),
    pixel: format::Pixel,
) -> Result<frame::Video> {
    if normalizer
        .as_ref()
        .is_none_or(|n| !n.matches(src, rotation))
    {
        *normalizer = Some(Normalizer::new(src, rotation, width, height, pixel, None)?);
    }
    normalizer.as_mut().expect("just built").run(src)
}

fn drain(
    enc: &mut encoder::Video,
    octx: &mut format::context::Output,
    enc_tb: ffmpeg::Rational,
    durations: &mut HashMap<i64, i64>,
) -> Result<()> {
    let stream_tb = octx.stream(0).expect("proxy stream").time_base();
    let mut packet = ffmpeg::Packet::empty();
    while enc.receive_packet(&mut packet).is_ok() {
        if let Some(d) = packet.pts().and_then(|pts| durations.remove(&pts)) {
            packet.set_duration(d);
        }
        packet.set_stream(0);
        packet.rescale_ts(enc_tb, stream_tb);
        packet
            .write_interleaved(octx)
            .ctx(|| "writing a proxy packet".into())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use EncoderChoice::{Nvenc, VideoToolbox, Vulkan};

    /// GPU encoders that work: their names, kinds and pixel formats.
    fn working(list: &[(&str, EncoderChoice, &str)]) -> Availability {
        let status = |&(encoder, kind, pixel): &(&str, EncoderChoice, &str)| {
            crate::export::encode::EncoderStatus {
                encoder: encoder.into(),
                kind,
                device: Some("0".into()),
                pixel: pixel.into(),
                ok: true,
                error: None,
            }
        };
        Availability {
            encoders: list.iter().map(status).collect(),
        }
    }

    fn names(choice: ProxyCodec, available: &Availability, plays: &[&str]) -> Vec<String> {
        crate::init();
        encoders(choice, available, plays)
            .into_iter()
            .map(|e| e.name)
            .collect()
    }

    #[test]
    fn prores_takes_apples_hardware_where_it_works() {
        let apple = working(&[("prores_videotoolbox", VideoToolbox, "nv12")]);
        let got = names(ProxyCodec::Prores, &apple, &[]);
        assert_eq!(got, ["prores_videotoolbox", "prores_ks"]);
    }

    #[test]
    fn prores_takes_vulkan_where_it_works() {
        let gpu = working(&[("prores_ks_vulkan", Vulkan, "yuv422p10le")]);
        let got = names(ProxyCodec::Prores, &gpu, &[]);
        assert_eq!(got, ["prores_ks_vulkan", "prores_ks"]);
    }

    #[test]
    fn automatic_takes_what_the_gpu_both_makes_and_plays() {
        let nvidia = working(&[
            ("av1_nvenc", Nvenc, "yuv420p"),
            ("h264_nvenc", Nvenc, "yuv420p"),
            ("prores_ks_vulkan", Vulkan, "yuv422p10le"),
        ]);
        let got = names(ProxyCodec::Auto, &nvidia, &["av1", "h264"]);
        assert_eq!(got[..2], ["av1_nvenc", "h264_nvenc"]);
    }

    #[test]
    fn automatic_takes_apples_prores_hardware() {
        let apple = working(&[
            ("h264_videotoolbox", VideoToolbox, "nv12"),
            ("prores_videotoolbox", VideoToolbox, "nv12"),
        ]);
        let got = names(ProxyCodec::Auto, &apple, &["h264", "prores"]);
        assert_eq!(got[0], "prores_videotoolbox");
    }

    #[test]
    fn automatic_puts_the_gpus_video_engine_before_vulkan() {
        let nvidia = working(&[
            ("av1_nvenc", Nvenc, "yuv420p"),
            ("prores_ks_vulkan", Vulkan, "yuv422p10le"),
        ]);
        let got = names(ProxyCodec::Auto, &nvidia, &["av1", "prores"]);
        assert_eq!(got[..2], ["av1_nvenc", "prores_ks_vulkan"]);
    }

    #[test]
    fn automatic_takes_what_the_gpu_plays_before_what_it_makes() {
        let gpu = working(&[("av1_nvenc", Nvenc, "yuv420p")]);
        let got = names(ProxyCodec::Auto, &gpu, &["prores"]);
        assert_eq!(got[..2], ["prores_ks", "av1_nvenc"]);
    }

    #[test]
    fn automatic_takes_what_the_gpu_makes_before_software() {
        let apple = working(&[("h264_videotoolbox", VideoToolbox, "nv12")]);
        let got = names(ProxyCodec::Auto, &apple, &[]);
        assert_eq!(got[0], "h264_videotoolbox");
    }

    #[test]
    fn automatic_without_a_gpu_makes_dnxhr_then_prores() {
        let got = names(ProxyCodec::Auto, &Availability::default(), &[]);
        assert_eq!(got[..2], ["dnxhd", "prores_ks"]);
    }
}
