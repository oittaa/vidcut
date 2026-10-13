//! Encoder selection: export settings → a concrete FFmpeg encoder, and which
//! GPU encoders actually work on this machine (by test-encoding).

use serde::{Deserialize, Serialize};
use vidcut_core::export::color::Abilities;
use vidcut_core::export::{
    AudioCodec, AudioSettings, EncoderChoice, FirstPass, Multipass, VideoCodec, VideoSettings,
};

use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, codec, encoder, ffi, format, frame};

/// A resolved video encoder.
#[derive(Debug, Clone)]
pub struct VideoEncoder {
    /// FFmpeg encoder name, e.g. `libx264`, `hevc_nvenc`, `h264_vaapi`.
    pub name: String,
    pub kind: EncoderChoice,
    /// Pixel format of the frames handed over (for VAAPI: the upload format).
    pub pixel: format::Pixel,
    /// VAAPI render node, e.g. `/dev/dri/renderD129`.
    pub device: Option<String>,
    pub options: Vec<(String, String)>,
    /// HDR output with this transfer; `None`: SDR BT.709.
    pub hdr: Option<vidcut_core::model::Hdr>,
    /// How it makes its passes (decision D45).
    pub passes: Passes,
    /// The static HDR metadata it writes (decision D46).
    pub metadata: crate::hdr::Static,
    /// The dynamic HDR metadata kept from the sources' frames.
    pub dynamic: crate::hdr::Dynamic,
}

/// The HDR metadata settings that have chosen already ask an encoder to
/// write (decision D46): none for SDR.
pub fn hdr_metadata(v: &VideoSettings) -> (crate::hdr::Static, crate::hdr::Dynamic) {
    use vidcut_core::export::{HdrMetadata, metadata};
    let Some(transfer) = v.hdr.transfer() else {
        return Default::default();
    };
    if v.hdr_metadata == HdrMetadata::None {
        return Default::default();
    }
    let pq = transfer == vidcut_core::model::Hdr::Pq;
    let stat = crate::hdr::Static {
        mastering: v
            .mastering_display
            .as_deref()
            .filter(|_| pq)
            .and_then(|m| metadata::parse_mastering(m).ok()),
        light: v
            .max_cll
            .as_deref()
            .filter(|_| pq)
            .and_then(|l| metadata::parse_light(l).ok()),
    };
    let dynamic = crate::hdr::Dynamic {
        hdr10plus: pq && v.hdr_metadata.hdr10plus(),
        dolby_vision: v.hdr_metadata.dolby_vision(),
    };
    (stat, dynamic)
}

/// Whether an encoder the settings allow writes Dolby Vision: libx265 and
/// libsvtav1, the CPU's.
pub fn writes_dolby_vision(v: &VideoSettings) -> bool {
    matches!(v.encoder, EncoderChoice::Auto | EncoderChoice::Cpu)
        && matches!(v.codec, VideoCodec::Hevc | VideoCodec::Av1)
}

/// How a video encoder makes its passes (decision D45).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Passes {
    One,
    /// The range is encoded twice: first to write statistics (nothing is
    /// muxed), then for real from them. x264, x265, VP9.
    TwoRuns {
        /// The first with faster settings (`video.first_pass = fast`).
        fast_first: bool,
    },
    /// Two passes over each frame within one encode: NVENC's `multipass`.
    TwoInside,
}

impl Passes {
    /// Passes over each frame.
    pub fn count(self) -> u8 {
        match self {
            Self::One => 1,
            Self::TwoRuns { .. } | Self::TwoInside => 2,
        }
    }

    /// Times the range is encoded.
    pub fn runs(self) -> u8 {
        match self {
            Self::TwoRuns { .. } => 2,
            Self::One | Self::TwoInside => 1,
        }
    }
}

/// Which pass an encoder is opened for.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Pass<'a> {
    /// The only one.
    Only,
    /// The first of two runs, writing statistics under `stats` (a file the
    /// encoder names itself, or nothing: some hand them back in memory).
    First { stats: &'a std::path::Path },
    /// The second, reading them: the same file, or the statistics the
    /// first handed back.
    Second {
        stats: &'a std::path::Path,
        buffer: Option<&'a str>,
    },
}

/// The passes `v` asks of an encoder `name`, or why it can't make them:
/// two with a target bitrate unless `passes` says otherwise.
fn passes(v: &VideoSettings, name: &str) -> Result<Passes> {
    let two = match v.passes {
        Some(n) => n == 2,
        None => v.bitrate.is_some(),
    };
    if !two {
        return Ok(Passes::One);
    }
    let fast_first = v.first_pass == FirstPass::Fast;
    match name {
        "libx264" | "libx265" | "libvpx-vp9" => Ok(Passes::TwoRuns { fast_first }),
        n if n.ends_with("_nvenc") => Ok(Passes::TwoInside),
        // Unset passes stay one where there's no second.
        _ if v.passes.is_none() => Ok(Passes::One),
        n if n.ends_with("_vaapi") => Err(Error::Invalid(format!(
            "{n} makes one pass; two need encoder cpu or nvenc"
        ))),
        "libsvtav1" => Err(Error::Invalid(
            "libsvtav1 makes one pass in this FFmpeg (its rate control looks ahead on its \
             own); for two, use encoder nvenc or another codec"
                .into(),
        )),
        n => Err(Error::Invalid(format!("{n} makes one pass"))),
    }
}

/// A resolved audio encoder.
#[derive(Debug, Clone)]
pub struct AudioEncoder {
    pub name: String,
    pub sample: format::Sample,
    pub rate: u32,
    pub channels: u16,
    pub bitrate: Option<u64>,
}

/// Whether a GPU encoder works, from a test encode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncoderStatus {
    pub encoder: String,
    pub kind: EncoderChoice,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Pixel format tested (8-bit `yuv420p`/`nv12`, 10-bit `p010le`).
    pub pixel: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// GPU encoder availability on this machine.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Availability {
    pub encoders: Vec<EncoderStatus>,
}

impl Availability {
    pub(crate) fn find(&self, name: &str, pixel: format::Pixel) -> Option<&EncoderStatus> {
        let pix = pixel_name(pixel);
        self.encoders
            .iter()
            .find(|e| e.ok && e.encoder == name && e.pixel == pix)
    }
}

/// FFmpeg's name for a pixel format, e.g. `p010le`.
pub fn pixel_name(p: format::Pixel) -> String {
    p.descriptor()
        .map(|d| d.name().to_string())
        .unwrap_or_default()
}

fn ten_bit(v: &VideoSettings) -> bool {
    v.bit_depth == Some(10) || v.profile.as_deref() == Some("main10") || v.hdr.transfer().is_some()
}

/// The CPU encoder of a codec.
fn cpu_name(codec: VideoCodec) -> &'static str {
    match codec {
        VideoCodec::H264 => "libx264",
        VideoCodec::Hevc => "libx265",
        VideoCodec::Av1 => "libsvtav1",
        VideoCodec::Vp9 => "libvpx-vp9",
        VideoCodec::Prores => "prores_ks",
        VideoCodec::Dnxhr => "dnxhd",
    }
}

/// Whether this FFmpeg's `name` encoder takes 10-bit pictures.
fn cpu_ten_bit(name: &str) -> bool {
    use format::Pixel;
    encoder::find_by_name(name)
        .and_then(|c| c.video().ok())
        .and_then(|v| v.formats())
        .is_some_and(|mut f| f.any(|p| matches!(p, Pixel::YUV420P10LE | Pixel::YUV422P10LE)))
}

/// What the encoders `v` may use can write (decision D44): whether any of
/// them writes 10 bits, and whether the one an 8-bit export starts with
/// does. For `auto` that is the first working GPU encoder (NVENC, then
/// VAAPI) and then the CPU's, as [`video_encoders`] tries them.
pub fn abilities(v: &VideoSettings, available: &Availability) -> Abilities {
    let kinds: &[EncoderChoice] = match v.encoder {
        EncoderChoice::Auto => &[
            EncoderChoice::Nvenc,
            EncoderChoice::Vaapi,
            EncoderChoice::Cpu,
        ],
        EncoderChoice::Cpu => &[EncoderChoice::Cpu],
        EncoderChoice::Nvenc => &[EncoderChoice::Nvenc],
        EncoderChoice::Vaapi => &[EncoderChoice::Vaapi],
    };
    let name = |kind: EncoderChoice| match kind {
        EncoderChoice::Cpu => Some(cpu_name(v.codec)),
        k => gpu_name(v.codec, k),
    };
    let writes = |kind: EncoderChoice, ten: bool| match (kind, name(kind)) {
        (_, None) => false,
        (EncoderChoice::Cpu, Some(n)) => {
            if ten {
                cpu_ten_bit(n)
            } else {
                encoder::find_by_name(n).is_some()
            }
        }
        (k, Some(n)) => available.find(n, gpu_pixel(k, ten)).is_some(),
    };
    let first = kinds.iter().copied().find(|&k| writes(k, false));
    Abilities {
        ten_bit: kinds.iter().any(|&k| writes(k, true)),
        ten_bit_first: first.is_some_and(|k| writes(k, true)),
        first: first.and_then(name),
    }
}

fn gpu_name(codec: VideoCodec, kind: EncoderChoice) -> Option<&'static str> {
    Some(match (codec, kind) {
        (VideoCodec::H264, EncoderChoice::Nvenc) => "h264_nvenc",
        (VideoCodec::Hevc, EncoderChoice::Nvenc) => "hevc_nvenc",
        (VideoCodec::Av1, EncoderChoice::Nvenc) => "av1_nvenc",
        (VideoCodec::H264, EncoderChoice::Vaapi) => "h264_vaapi",
        (VideoCodec::Hevc, EncoderChoice::Vaapi) => "hevc_vaapi",
        (VideoCodec::Av1, EncoderChoice::Vaapi) => "av1_vaapi",
        _ => return None,
    })
}

/// The frame format a GPU encoder kind takes for 8- or 10-bit output.
fn gpu_pixel(kind: EncoderChoice, ten: bool) -> format::Pixel {
    match (kind, ten) {
        (EncoderChoice::Vaapi, false) => format::Pixel::NV12,
        (_, true) => format::Pixel::P010LE,
        _ => format::Pixel::YUV420P,
    }
}

fn opt(k: &str, v: impl ToString) -> (String, String) {
    (k.to_string(), v.to_string())
}

/// Resolves video settings to the encoder an export starts with: for
/// `auto`, a working GPU encoder (NVENC, then VAAPI), else the CPU encoder.
pub fn resolve_video(v: &VideoSettings, available: &Availability) -> Result<VideoEncoder> {
    let mut errors = Vec::new();
    for candidate in video_encoders(v, available) {
        match candidate {
            Ok(e) => return Ok(e),
            Err(e) => errors.push(e),
        }
    }
    Err(errors.remove(0))
}

/// The encoders that may encode `v`, in order: the one asked for, or for
/// `auto` every working GPU encoder (NVENC, then VAAPI) and then the CPU
/// encoder; with, in its place, why one can't. An export takes the first
/// that opens: a GPU encoder can fail to at that moment (NVENC allows a
/// few sessions at a time, and other exports may hold them).
///
/// An explicit `speed` is for the encoder an export starts with. Encoders
/// don't share speed names (x264's, NVENC's p1–p7, SVT-AV1's numbers), so
/// one it moves on to uses its own default.
pub fn video_encoders(v: &VideoSettings, available: &Availability) -> Vec<Result<VideoEncoder>> {
    let kinds: Vec<EncoderChoice> = match v.encoder {
        // Dolby Vision is written by the CPU's encoders only.
        EncoderChoice::Auto if v.hdr_metadata.dolby_vision() => vec![EncoderChoice::Cpu],
        EncoderChoice::Auto => vec![
            EncoderChoice::Nvenc,
            EncoderChoice::Vaapi,
            EncoderChoice::Cpu,
        ],
        k => vec![k],
    };
    let fallback = VideoSettings {
        speed: None,
        ..v.clone()
    };
    let mut found: Vec<Result<VideoEncoder>> = Vec::new();
    for kind in kinds {
        let settings = if found.iter().any(Result::is_ok) {
            &fallback
        } else {
            v
        };
        match video_encoder(settings, kind, v.encoder == kind, available) {
            Ok(Some(e)) => found.push(Ok(e)),
            Ok(None) => {}
            Err(e) => found.push(Err(e)),
        }
    }
    if found.is_empty() {
        found.push(Err(Error::Invalid(format!("no encoder for {:?}", v.codec))));
    }
    found
}

/// The `kind` encoder for `v`, or `None` when this machine has none that
/// works and it wasn't `asked` for by name.
fn video_encoder(
    v: &VideoSettings,
    kind: EncoderChoice,
    asked: bool,
    available: &Availability,
) -> Result<Option<VideoEncoder>> {
    let ten = ten_bit(v);
    if kind == EncoderChoice::Cpu {
        return cpu_video(v, ten).map(Some);
    }
    let Some(name) = gpu_name(v.codec, kind) else {
        return Ok(None);
    };
    let pixel = gpu_pixel(kind, ten);
    let Some(status) = available.find(name, pixel) else {
        if asked {
            return Err(Error::Invalid(format!(
                "{name} ({}) is not available on this machine; see `vidcut encoders list`",
                pixel_name(pixel)
            )));
        }
        return Ok(None);
    };
    let q = v.quality.unwrap_or(match v.codec {
        VideoCodec::Av1 => 30,
        _ => 20,
    });
    let passes = passes(v, name)?;
    let mut options = Vec::new();
    if kind == EncoderChoice::Nvenc {
        options.push(opt("preset", v.speed.as_deref().unwrap_or("p5")));
        options.push(opt("tune", "hq"));
        options.push(opt("rc", "vbr"));
        if passes == Passes::TwoInside {
            let first = match v.multipass {
                Multipass::Quarter => "qres",
                Multipass::Full => "fullres",
            };
            options.push(opt("multipass", first));
        }
        let lookahead = v
            .lookahead
            .unwrap_or(vidcut_core::export::DEFAULT_LOOKAHEAD);
        if lookahead > 0 {
            options.push(opt("rc-lookahead", lookahead));
        }
        match v.bitrate {
            Some(b) => {
                options.push(opt("b", b));
                options.push(opt("maxrate", b * 3 / 2));
            }
            None => {
                options.push(opt("cq", q));
                options.push(opt("b", 0));
            }
        }
        if v.codec == VideoCodec::Hevc {
            options.push(opt("profile", if ten { "main10" } else { "main" }));
        }
        if v.codec == VideoCodec::H264 {
            options.push(opt("profile", v.profile.as_deref().unwrap_or("high")));
        }
    } else {
        match v.bitrate {
            Some(b) => {
                options.push(opt("rc_mode", "VBR"));
                options.push(opt("b", b));
            }
            None => {
                options.push(opt("rc_mode", "CQP"));
                options.push(opt("qp", q));
            }
        }
    }
    let (metadata, dynamic) = hdr_metadata(v);
    Ok(Some(VideoEncoder {
        name: name.into(),
        kind,
        pixel,
        device: status.device.clone(),
        options,
        hdr: v.hdr.transfer(),
        passes,
        metadata,
        dynamic,
    }))
}

fn cpu_video(v: &VideoSettings, ten: bool) -> Result<VideoEncoder> {
    use format::Pixel;
    let (metadata, dynamic) = hdr_metadata(v);
    // HDR10's SEI at every keyframe, not only in the stream's header, and
    // x265's QP adjustment for PQ.
    let x265 = {
        let mut params = "log-level=error".to_string();
        if !metadata.is_empty() || dynamic.hdr10plus || dynamic.dolby_vision {
            params += ":repeat-headers=1";
        }
        if v.hdr.transfer() == Some(vidcut_core::model::Hdr::Pq) {
            params += ":hdr10-opt=1";
        }
        params
    };
    let speed = |default: &str| opt("preset", v.speed.as_deref().unwrap_or(default));
    let quality = |key: &str, default: u32| match v.bitrate {
        Some(b) => opt("b", b),
        None => opt(key, v.quality.unwrap_or(default)),
    };
    let (name, pixel, options) = match v.codec {
        VideoCodec::H264 => {
            // Quality 0 is lossless, which only the High 4:4:4 Predictive profile allows.
            let profile = match (ten, v.quality == Some(0) && v.bitrate.is_none()) {
                (_, true) => "high444",
                (true, false) => "high10",
                (false, false) => v.profile.as_deref().unwrap_or("high"),
            };
            (
                "libx264",
                if ten {
                    Pixel::YUV420P10LE
                } else {
                    Pixel::YUV420P
                },
                vec![speed("medium"), quality("crf", 18), opt("profile", profile)],
            )
        }
        VideoCodec::Hevc => (
            "libx265",
            if ten {
                Pixel::YUV420P10LE
            } else {
                Pixel::YUV420P
            },
            {
                let mut o = vec![
                    speed("medium"),
                    quality("crf", 20),
                    opt("x265-params", x265),
                ];
                if dynamic.dolby_vision {
                    o.push(opt("dolbyvision", 1));
                }
                o
            },
        ),
        VideoCodec::Av1 => (
            "libsvtav1",
            if ten {
                Pixel::YUV420P10LE
            } else {
                Pixel::YUV420P
            },
            {
                let mut o = vec![speed("8"), quality("crf", 30)];
                if dynamic.dolby_vision {
                    o.push(opt("dolbyvision", 1));
                }
                o
            },
        ),
        VideoCodec::Vp9 => (
            "libvpx-vp9",
            if ten {
                Pixel::YUV420P10LE
            } else {
                Pixel::YUV420P
            },
            {
                let mut o = vec![
                    quality("crf", 32),
                    opt("row-mt", 1),
                    opt("cpu-used", v.speed.as_deref().unwrap_or("4")),
                ];
                if v.bitrate.is_none() {
                    o.push(opt("b", 0));
                }
                o
            },
        ),
        VideoCodec::Prores => {
            let (profile, pixel) = match v.profile.as_deref().unwrap_or("hq") {
                "proxy" => (0, Pixel::YUV422P10LE),
                "lt" => (1, Pixel::YUV422P10LE),
                "standard" => (2, Pixel::YUV422P10LE),
                "4444" => (4, Pixel::YUVA444P10LE),
                _ => (3, Pixel::YUV422P10LE),
            };
            (
                "prores_ks",
                pixel,
                vec![opt("profile", profile), opt("vendor", "apl0")],
            )
        }
        VideoCodec::Dnxhr => {
            let p = v.profile.as_deref().unwrap_or("hq");
            let pixel = match p {
                "hqx" => Pixel::YUV422P10LE,
                "444" => Pixel::YUV444P10LE,
                _ => Pixel::YUV422P,
            };
            ("dnxhd", pixel, vec![opt("profile", format!("dnxhr_{p}"))])
        }
    };
    if encoder::find_by_name(name).is_none() {
        return Err(Error::Invalid(format!("this FFmpeg has no {name} encoder")));
    }
    Ok(VideoEncoder {
        name: name.into(),
        kind: EncoderChoice::Cpu,
        pixel,
        device: None,
        options,
        hdr: v.hdr.transfer(),
        passes: passes(v, name)?,
        metadata,
        dynamic,
    })
}

/// Resolves audio settings to an encoder at the given default rate/channels.
pub fn resolve_audio(a: &AudioSettings, rate: u32, channels: u16) -> Result<AudioEncoder> {
    use format::Sample;
    use format::sample::Type::{Packed, Planar};
    let (name, sample, default_kbps) = match a.codec {
        AudioCodec::Aac => ("aac", Sample::F32(Planar), Some(256)),
        AudioCodec::Opus => ("libopus", Sample::F32(Packed), Some(160)),
        AudioCodec::Mp3 => ("libmp3lame", Sample::F32(Planar), Some(320)),
        AudioCodec::Flac => ("flac", Sample::I32(Packed), None),
        AudioCodec::PcmS16 => ("pcm_s16le", Sample::I16(Packed), None),
        AudioCodec::PcmS24 => ("pcm_s24le", Sample::I32(Packed), None),
    };
    if encoder::find_by_name(name).is_none() {
        return Err(Error::Invalid(format!("this FFmpeg has no {name} encoder")));
    }
    let mut rate = a.sample_rate.unwrap_or(rate);
    if a.codec == AudioCodec::Opus {
        rate = 48_000; // Opus only runs at 48 kHz here.
    }
    Ok(AudioEncoder {
        name: name.into(),
        sample,
        rate,
        channels: a.channels.unwrap_or(channels),
        bitrate: a.bitrate.or(default_kbps.map(|k| k * 1000)),
    })
}

/// A VAAPI device and frame pool for uploading frames to the GPU encoder.
pub(crate) struct Vaapi {
    device: *mut ffi::AVBufferRef,
    pub(crate) frames: *mut ffi::AVBufferRef,
}

impl Drop for Vaapi {
    fn drop(&mut self) {
        unsafe {
            ffi::av_buffer_unref(&mut self.frames);
            ffi::av_buffer_unref(&mut self.device);
        }
    }
}

impl Vaapi {
    pub(crate) fn new(node: &str, sw: format::Pixel, width: u32, height: u32) -> Result<Self> {
        let path =
            std::ffi::CString::new(node).map_err(|_| Error::Invalid("bad device path".into()))?;
        unsafe {
            let mut device = std::ptr::null_mut();
            let ret = ffi::av_hwdevice_ctx_create(
                &mut device,
                ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
                path.as_ptr(),
                std::ptr::null_mut(),
                0,
            );
            if ret < 0 {
                return Err(Error::Ffmpeg {
                    context: format!("opening VAAPI device {node}"),
                    source: ffmpeg::Error::from(ret),
                });
            }
            let mut frames = ffi::av_hwframe_ctx_alloc(device);
            if frames.is_null() {
                ffi::av_buffer_unref(&mut device);
                return Err(Error::Invalid("cannot allocate VAAPI frames".into()));
            }
            let fc = &mut *((*frames).data as *mut ffi::AVHWFramesContext);
            fc.format = ffi::AVPixelFormat::AV_PIX_FMT_VAAPI;
            fc.sw_format = sw.into();
            fc.width = width as i32;
            fc.height = height as i32;
            fc.initial_pool_size = 20;
            let ret = ffi::av_hwframe_ctx_init(frames);
            if ret < 0 {
                ffi::av_buffer_unref(&mut frames);
                ffi::av_buffer_unref(&mut device);
                return Err(Error::Ffmpeg {
                    context: format!("setting up VAAPI frames on {node}"),
                    source: ffmpeg::Error::from(ret),
                });
            }
            Ok(Self { device, frames })
        }
    }

    /// Copies a system-memory frame into a GPU frame (keeping its pts).
    pub(crate) fn upload(&self, src: &frame::Video) -> Result<frame::Video> {
        let mut hw = frame::Video::empty();
        unsafe {
            let ret = ffi::av_hwframe_get_buffer(self.frames, hw.as_mut_ptr(), 0);
            if ret < 0 {
                return Err(Error::Ffmpeg {
                    context: "getting a VAAPI frame".into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
            let ret = ffi::av_hwframe_transfer_data(hw.as_mut_ptr(), src.as_ptr(), 0);
            if ret < 0 {
                return Err(Error::Ffmpeg {
                    context: "uploading to VAAPI".into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
        }
        hw.set_pts(src.pts());
        Ok(hw)
    }
}

/// Opens a video encoder for `enc` at `width`×`height` and `fps`.
/// Returns the opened encoder and, for VAAPI, the upload context.
/// With `gpu_frames` (NVENC), the encoder takes frames in GPU memory of
/// that frames context.
pub(crate) fn open_video(
    enc: &VideoEncoder,
    width: u32,
    height: u32,
    fps: ffmpeg::Rational,
    global_header: bool,
    gpu_frames: Option<*mut ffi::AVBufferRef>,
    pass: Pass<'_>,
) -> Result<(encoder::Video, Option<Vaapi>)> {
    let codec = encoder::find_by_name(&enc.name)
        .ok_or_else(|| Error::Invalid(format!("no {} encoder", enc.name)))?;
    let mut ctx = codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()
        .ctx(|| format!("creating {}", enc.name))?;
    ctx.set_width(width);
    ctx.set_height(height);
    ctx.set_time_base(fps.invert());
    ctx.set_frame_rate(Some(fps));
    crate::color::tag(unsafe { ctx.as_mut_ptr() }, enc.hdr);
    if enc.hdr.is_some() && !enc.metadata.is_empty() {
        unsafe { crate::hdr::set_on_encoder(ctx.as_mut_ptr(), &enc.metadata)? };
    }
    let mut flags = codec::Flags::empty();
    if global_header {
        flags |= codec::Flags::GLOBAL_HEADER;
    }
    match pass {
        Pass::Only => {}
        Pass::First { .. } => flags |= codec::Flags::PASS1,
        Pass::Second { .. } => flags |= codec::Flags::PASS2,
    }
    ctx.set_flags(flags);
    let vaapi = match &enc.device {
        _ if gpu_frames.is_some() => {
            ctx.set_format(format::Pixel::CUDA);
            let frames = unsafe { ffi::av_buffer_ref(gpu_frames.expect("checked")) };
            if frames.is_null() {
                return Err(Error::Invalid("no memory for the GPU frames".into()));
            }
            unsafe { (*ctx.as_mut_ptr()).hw_frames_ctx = frames };
            None
        }
        Some(node) if enc.kind == EncoderChoice::Vaapi => {
            let v = Vaapi::new(node, enc.pixel, width, height)?;
            ctx.set_format(format::Pixel::VAAPI);
            let frames = unsafe { ffi::av_buffer_ref(v.frames) };
            if frames.is_null() {
                return Err(Error::Invalid("no memory for the VAAPI frames".into()));
            }
            unsafe { (*ctx.as_mut_ptr()).hw_frames_ctx = frames };
            Some(v)
        }
        _ => {
            ctx.set_format(enc.pixel);
            None
        }
    };
    let mut dict = ffmpeg::Dictionary::new();
    for (k, v) in pass_options(enc, pass) {
        dict.set(&k, &v);
    }
    // The first run's statistics in memory (libvpx), read once opened.
    if let Pass::Second {
        buffer: Some(stats),
        ..
    } = pass
    {
        unsafe {
            let raw = ctx.as_mut_ptr();
            ffi::av_freep(&mut (*raw).stats_in as *mut *mut std::ffi::c_char as *mut _);
            let copy = ffi::av_malloc(stats.len() + 1) as *mut u8;
            if copy.is_null() {
                return Err(Error::Invalid(
                    "no memory for the first pass's statistics".into(),
                ));
            }
            std::ptr::copy_nonoverlapping(stats.as_ptr(), copy, stats.len());
            *copy.add(stats.len()) = 0;
            (*raw).stats_in = copy as *mut std::ffi::c_char;
        }
    }
    let opened = ctx.open_as_with(codec, dict);
    let mut opened = opened.ctx(|| format!("opening {}", enc.name))?;
    unsafe {
        // Read when the encoder opened; ours to free.
        let raw = opened.as_mut_ptr();
        ffi::av_freep(&mut (*raw).stats_in as *mut *mut std::ffi::c_char as *mut _);
    }
    Ok((opened, vaapi))
}

/// The encoder's options for `pass`: the statistics file and the first
/// run's speed where it has its own.
fn pass_options(enc: &VideoEncoder, pass: Pass<'_>) -> Vec<(String, String)> {
    let mut options = enc.options.clone();
    let fast = matches!(enc.passes, Passes::TwoRuns { fast_first: true });
    let set = |options: &mut Vec<(String, String)>, k: &str, v: String| {
        options.retain(|(key, _)| key != k);
        options.push((k.to_string(), v));
    };
    let (stats, first) = match pass {
        Pass::Only => return options,
        Pass::First { stats } => (stats, true),
        Pass::Second { stats, .. } => (stats, false),
    };
    match enc.name.as_str() {
        "libx264" => {
            set(&mut options, "stats", stats.display().to_string());
            if first {
                set(&mut options, "fastfirstpass", u8::from(fast).to_string());
            }
        }
        "libx265" => {
            // x265 takes its passes in x265-params, `:`-separated `k=v`.
            let escaped: String = stats
                .display()
                .to_string()
                .chars()
                .flat_map(|c| match c {
                    ':' | '=' | '\\' | '\'' => vec!['\\', c],
                    c => vec![c],
                })
                .collect();
            let mut params = options
                .iter()
                .find(|(k, _)| k == "x265-params")
                .map_or(String::new(), |(_, v)| format!("{v}:"));
            params += &format!("pass={}:stats={escaped}", if first { 1 } else { 2 });
            if first && fast {
                params += ":slow-firstpass=0";
            }
            set(&mut options, "x265-params", params);
        }
        "libvpx-vp9" if first && fast => {
            // libvpx's measuring pass at speed 4, unless the encode is faster.
            let speed = options
                .iter()
                .find(|(k, _)| k == "cpu-used")
                .and_then(|(_, v)| v.parse::<i32>().ok())
                .unwrap_or(0);
            set(&mut options, "cpu-used", speed.max(4).to_string());
        }
        _ => {}
    }
    options
}

/// Test-encodes a few frames to see whether a GPU encoder works.
fn try_gpu(
    name: &str,
    kind: EncoderChoice,
    pixel: format::Pixel,
    device: Option<&str>,
) -> Result<()> {
    let enc = VideoEncoder {
        name: name.into(),
        kind,
        pixel,
        device: device.map(String::from),
        options: Vec::new(),
        hdr: None,
        passes: Passes::One,
        metadata: Default::default(),
        dynamic: Default::default(),
    };
    let (w, h) = (640, 360);
    let (mut encoder, vaapi) =
        open_video(&enc, w, h, ffmpeg::Rational(30, 1), false, None, Pass::Only)?;
    let mut sw = frame::Video::new(pixel, w, h);
    crate::export::video::fill_black(&mut sw)?;
    let mut packets = 0;
    for i in 0..3 {
        sw.set_pts(Some(i));
        let f = match &vaapi {
            Some(v) => v.upload(&sw)?,
            None => crate::export::video::frame_ref(&sw)?,
        };
        encoder.send_frame(&f).ctx(|| format!("{name}: encoding"))?;
        let mut p = ffmpeg::Packet::empty();
        while encoder.receive_packet(&mut p).is_ok() {
            packets += 1;
        }
    }
    encoder.send_eof().ctx(|| format!("{name}: flushing"))?;
    let mut p = ffmpeg::Packet::empty();
    while encoder.receive_packet(&mut p).is_ok() {
        packets += 1;
    }
    if packets == 0 {
        return Err(Error::Invalid(format!("{name} produced no output")));
    }
    Ok(())
}

/// VAAPI render nodes (`/dev/dri/renderD*`) with their kernel driver names.
pub fn render_nodes() -> Vec<(String, String)> {
    let mut nodes: Vec<(String, String)> = std::fs::read_dir("/sys/class/drm")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("renderD"))
        .map(|e| {
            let driver = std::fs::read_link(e.path().join("device/driver"))
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_default();
            (
                format!("/dev/dri/{}", e.file_name().to_string_lossy()),
                driver,
            )
        })
        .collect();
    nodes.sort();
    nodes
}

/// Test-encodes every GPU encoder and pixel format the presets can use.
pub fn detect() -> Availability {
    crate::init();
    let mut encoders = Vec::new();
    let mut check =
        |name: &str, kind: EncoderChoice, pixel: format::Pixel, device: Option<&str>| {
            let result = if encoder::find_by_name(name).is_none() {
                Err(Error::Invalid(format!("{name} not built into this FFmpeg")))
            } else {
                try_gpu(name, kind, pixel, device)
            };
            encoders.push(EncoderStatus {
                encoder: name.into(),
                kind,
                device: device.map(String::from),
                pixel: pixel_name(pixel),
                ok: result.is_ok(),
                error: result.err().map(|e| e.to_string()),
            });
        };
    for (codec, pixels) in [
        ("h264_nvenc", &[format::Pixel::YUV420P][..]),
        (
            "hevc_nvenc",
            &[format::Pixel::YUV420P, format::Pixel::P010LE],
        ),
        (
            "av1_nvenc",
            &[format::Pixel::YUV420P, format::Pixel::P010LE],
        ),
    ] {
        for &p in pixels {
            check(codec, EncoderChoice::Nvenc, p, None);
        }
    }
    for (node, _driver) in render_nodes() {
        for (codec, pixels) in [
            ("h264_vaapi", &[format::Pixel::NV12][..]),
            ("hevc_vaapi", &[format::Pixel::NV12, format::Pixel::P010LE]),
            ("av1_vaapi", &[format::Pixel::NV12, format::Pixel::P010LE]),
        ] {
            for &p in pixels {
                check(codec, EncoderChoice::Vaapi, p, Some(&node));
            }
        }
    }
    Availability { encoders }
}

#[cfg(test)]
mod tests {
    use vidcut_core::export::{HdrMode, Multipass};

    use super::*;

    fn video(codec: VideoCodec, encoder: EncoderChoice, bitrate: Option<u64>) -> VideoSettings {
        VideoSettings {
            codec,
            encoder,
            quality: None,
            bitrate,
            speed: None,
            profile: None,
            bit_depth: Some(8),
            width: None,
            height: None,
            hdr: HdrMode::Sdr,
            passes: None,
            first_pass: FirstPass::Fast,
            multipass: Multipass::Quarter,
            lookahead: None,
            hdr_metadata: Default::default(),
            mastering_display: None,
            max_cll: None,
        }
    }

    fn nvenc_here() -> Availability {
        Availability {
            encoders: ["h264_nvenc", "hevc_vaapi"]
                .into_iter()
                .map(|name| EncoderStatus {
                    encoder: name.into(),
                    kind: if name.ends_with("nvenc") {
                        EncoderChoice::Nvenc
                    } else {
                        EncoderChoice::Vaapi
                    },
                    device: name
                        .ends_with("vaapi")
                        .then(|| "/dev/dri/renderD129".into()),
                    pixel: if name.ends_with("nvenc") {
                        "yuv420p".into()
                    } else {
                        "nv12".into()
                    },
                    ok: true,
                    error: None,
                })
                .collect(),
        }
    }

    #[test]
    fn passes_follow_the_rate_control() {
        // Decision D45: two with a bitrate where the encoder makes them.
        crate::init();
        let none = Availability::default();
        let passes = |v: &VideoSettings, a: &Availability| resolve_video(v, a).map(|e| e.passes);
        let cpu = |codec, bitrate| video(codec, EncoderChoice::Cpu, bitrate);
        for codec in [VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Vp9] {
            assert_eq!(
                passes(&cpu(codec, Some(4_000_000)), &none).unwrap(),
                Passes::TwoRuns { fast_first: true },
                "{codec:?}"
            );
            assert_eq!(passes(&cpu(codec, None), &none).unwrap(), Passes::One);
        }
        // SVT-AV1 makes one here: unset passes are one, two are an error.
        let mut av1 = cpu(VideoCodec::Av1, Some(4_000_000));
        assert_eq!(passes(&av1, &none).unwrap(), Passes::One);
        av1.passes = Some(2);
        assert!(passes(&av1, &none).is_err());
        // One asked for: one.
        let mut one = cpu(VideoCodec::H264, Some(4_000_000));
        one.passes = Some(1);
        assert_eq!(passes(&one, &none).unwrap(), Passes::One);
        // NVENC looks twice inside one encode, at a quarter of the size
        // unless told; VAAPI makes one.
        let mut nvenc = video(VideoCodec::H264, EncoderChoice::Auto, Some(4_000_000));
        let e = resolve_video(&nvenc, &nvenc_here()).unwrap();
        assert_eq!(
            (e.name.as_str(), e.passes),
            ("h264_nvenc", Passes::TwoInside)
        );
        assert!(
            e.options.contains(&opt("multipass", "qres")),
            "{:?}",
            e.options
        );
        // It looks 32 frames ahead unless told otherwise, 0 not at all.
        assert!(e.options.contains(&opt("rc-lookahead", 32)));
        nvenc.multipass = Multipass::Full;
        nvenc.lookahead = Some(16);
        let e = resolve_video(&nvenc, &nvenc_here()).unwrap();
        assert!(e.options.contains(&opt("multipass", "fullres")));
        assert!(e.options.contains(&opt("rc-lookahead", 16)));
        nvenc.lookahead = Some(0);
        nvenc.bitrate = None;
        let e = resolve_video(&nvenc, &nvenc_here()).unwrap();
        assert_eq!(e.passes, Passes::One);
        assert!(
            !e.options
                .iter()
                .any(|(k, _)| k == "rc-lookahead" || k == "multipass")
        );
        let mut vaapi = video(VideoCodec::Hevc, EncoderChoice::Vaapi, Some(4_000_000));
        assert_eq!(passes(&vaapi, &nvenc_here()).unwrap(), Passes::One);
        vaapi.passes = Some(2);
        assert!(passes(&vaapi, &nvenc_here()).is_err());
        // With `auto`, an encoder that can't make two gives way to one
        // that can.
        vaapi.encoder = EncoderChoice::Auto;
        let e = resolve_video(&vaapi, &nvenc_here()).unwrap();
        assert_eq!((e.name.as_str(), e.passes.runs()), ("libx265", 2));
    }

    #[test]
    fn each_pass_gets_its_statistics() {
        crate::init();
        let stats = std::path::Path::new("/tmp/a:b=c/stats");
        let enc = |codec| {
            resolve_video(
                &video(codec, EncoderChoice::Cpu, Some(1_000_000)),
                &Availability::default(),
            )
            .unwrap()
        };
        let x264 = enc(VideoCodec::H264);
        let first = pass_options(&x264, Pass::First { stats });
        assert!(first.contains(&opt("stats", stats.display())));
        assert!(first.contains(&opt("fastfirstpass", 1)));
        let second = pass_options(
            &x264,
            Pass::Second {
                stats,
                buffer: None,
            },
        );
        assert!(second.contains(&opt("stats", stats.display())));
        assert!(!second.iter().any(|(k, _)| k == "fastfirstpass"));
        // x265 in its parameters, the path's separators escaped.
        let x265 = enc(VideoCodec::Hevc);
        let params = |options: Vec<(String, String)>| {
            options
                .into_iter()
                .find(|(k, _)| k == "x265-params")
                .unwrap()
                .1
        };
        assert_eq!(
            params(pass_options(&x265, Pass::First { stats })),
            r"log-level=error:pass=1:stats=/tmp/a\:b\=c/stats:slow-firstpass=0"
        );
        assert_eq!(
            params(pass_options(
                &x265,
                Pass::Second {
                    stats,
                    buffer: None
                }
            )),
            r"log-level=error:pass=2:stats=/tmp/a\:b\=c/stats"
        );
        assert_eq!(pass_options(&x265, Pass::Only), x265.options);
    }
}
