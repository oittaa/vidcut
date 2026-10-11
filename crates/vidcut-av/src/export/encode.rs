//! Encoder selection: export settings → a concrete FFmpeg encoder, and which
//! GPU encoders actually work on this machine (by test-encoding).

use serde::{Deserialize, Serialize};
use vidcut_core::export::{AudioCodec, AudioSettings, EncoderChoice, VideoCodec, VideoSettings};

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
    fn find(&self, name: &str, pixel: format::Pixel) -> Option<&EncoderStatus> {
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
    v.bit_depth == Some(10) || v.profile.as_deref() == Some("main10") || v.hdr.is_some()
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

/// Resolves video settings to an encoder, preferring a working GPU encoder
/// for `auto` (NVENC, then VAAPI), else the CPU encoder.
pub fn resolve_video(v: &VideoSettings, available: &Availability) -> Result<VideoEncoder> {
    let ten = ten_bit(v);
    let kinds: Vec<EncoderChoice> = match v.encoder {
        EncoderChoice::Auto => vec![
            EncoderChoice::Nvenc,
            EncoderChoice::Vaapi,
            EncoderChoice::Cpu,
        ],
        k => vec![k],
    };
    for kind in kinds {
        if kind == EncoderChoice::Cpu {
            return cpu_video(v, ten);
        }
        let Some(name) = gpu_name(v.codec, kind) else {
            continue;
        };
        let pixel = gpu_pixel(kind, ten);
        let Some(status) = available.find(name, pixel) else {
            if v.encoder == kind {
                return Err(Error::Invalid(format!(
                    "{name} ({}) is not available on this machine; see `vidcut encoders`",
                    pixel_name(pixel)
                )));
            }
            continue;
        };
        let q = v.quality.unwrap_or(match v.codec {
            VideoCodec::Av1 => 30,
            _ => 20,
        });
        let mut options = Vec::new();
        if kind == EncoderChoice::Nvenc {
            options.push(opt("preset", v.speed.as_deref().unwrap_or("p5")));
            options.push(opt("tune", "hq"));
            options.push(opt("rc", "vbr"));
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
        return Ok(VideoEncoder {
            name: name.into(),
            kind,
            pixel,
            device: status.device.clone(),
            options,
            hdr: v.hdr,
        });
    }
    Err(Error::Invalid(format!("no encoder for {:?}", v.codec)))
}

fn cpu_video(v: &VideoSettings, ten: bool) -> Result<VideoEncoder> {
    use format::Pixel;
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
            vec![
                speed("medium"),
                quality("crf", 20),
                opt("x265-params", "log-level=error"),
            ],
        ),
        VideoCodec::Av1 => (
            "libsvtav1",
            if ten {
                Pixel::YUV420P10LE
            } else {
                Pixel::YUV420P
            },
            vec![speed("8"), quality("crf", 30)],
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
        hdr: v.hdr,
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
pub(crate) fn open_video(
    enc: &VideoEncoder,
    width: u32,
    height: u32,
    fps: ffmpeg::Rational,
    global_header: bool,
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
    if global_header {
        ctx.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    let vaapi = match &enc.device {
        Some(node) if enc.kind == EncoderChoice::Vaapi => {
            let v = Vaapi::new(node, enc.pixel, width, height)?;
            ctx.set_format(format::Pixel::VAAPI);
            unsafe { (*ctx.as_mut_ptr()).hw_frames_ctx = ffi::av_buffer_ref(v.frames) };
            Some(v)
        }
        _ => {
            ctx.set_format(enc.pixel);
            None
        }
    };
    let mut dict = ffmpeg::Dictionary::new();
    for (k, v) in &enc.options {
        dict.set(k, v);
    }
    let opened = ctx
        .open_as_with(codec, dict)
        .ctx(|| format!("opening {}", enc.name))?;
    Ok((opened, vaapi))
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
    };
    let (w, h) = (640, 360);
    let (mut encoder, vaapi) = open_video(&enc, w, h, ffmpeg::Rational(30, 1), false)?;
    let mut sw = frame::Video::new(pixel, w, h);
    crate::export::video::fill_black(&mut sw)?;
    let mut packets = 0;
    for i in 0..3 {
        sw.set_pts(Some(i));
        let f = match &vaapi {
            Some(v) => v.upload(&sw)?,
            None => crate::export::video::frame_ref(&sw),
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
