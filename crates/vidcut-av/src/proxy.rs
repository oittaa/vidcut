//! Playback proxies: a small, easy-to-decode copy of a source's pictures,
//! AV1 or H.264 (decision D41).
//!
//! Each proxy frame keeps the source frame's presentation timestamp, in the
//! source time base when that base is ordinary and in microseconds otherwise.
//! The preview can then swap the file and keep the same EDL `start` math
//! (decision D41). Audio is not copied: preview audio stays on the original.
//!
//! The file is Matroska, written to a temporary name and renamed into place.
//! Pictures are turned upright and scaled to `width`×`height` as 8-bit 4:2:0.
//! Every encoder makes a keyframe every 60 frames and shows frames in the
//! order it codes them: a seek decodes at most 60 frames, and the preview
//! needs no tail patches for a proxy.

use std::collections::HashMap;
use std::path::Path;

use vidcut_core::export::EncoderChoice;
use vidcut_core::preferences::ProxyCodec;

use crate::decode::{Input, VideoReader};
use crate::error::{Context, Error, Result};
use crate::export::encode::{Availability, Vaapi};
use crate::export::video::Normalizer;
use crate::ffmpeg::{self, codec, encoder, ffi, format, frame};

/// Frames between keyframes.
const KEYFRAMES: u32 = 60;

/// An encoder proxies can be made with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyEncoder {
    /// FFmpeg's encoder: `av1_nvenc`, `av1_vaapi`, `libsvtav1` or `libx264`.
    pub name: String,
    /// `av1` or `h264`.
    pub codec: &'static str,
    /// Hardware (NVENC, VAAPI) or software (CPU).
    pub kind: EncoderChoice,
    /// A VAAPI render node.
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
            _ => "H.264",
        };
        let what = match (self.kind, self.name.as_str()) {
            (EncoderChoice::Nvenc, _) => "NVIDIA hardware (NVENC)".to_string(),
            (EncoderChoice::Vaapi, _) => format!(
                "VAAPI hardware ({})",
                self.device.as_deref().unwrap_or("GPU")
            ),
            (_, "libsvtav1") => "software (SVT-AV1)".to_string(),
            (_, "libx264") => "software (x264)".to_string(),
            (_, other) => format!("software ({other})"),
        };
        format!("{codec}, {what}")
    }

    fn pixel(&self) -> format::Pixel {
        match self.kind {
            EncoderChoice::Vaapi => format::Pixel::NV12,
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
            "av1_nvenc" => vec![
                ("preset", "p4".into()),
                ("tune", "hq".into()),
                ("rc", "vbr".into()),
                ("cq", "32".into()),
                ("b", "0".into()),
            ],
            // AV1's quantizer index runs 0..255.
            "av1_vaapi" => vec![("rc_mode", "CQP".into()), ("qp", "128".into())],
            _ => Vec::new(),
        }
    }
}

/// The encoders `choice` makes proxies with here, best first: hardware AV1
/// (NVENC, then VAAPI) when this machine has a working one, then x264 for
/// automatic, SVT-AV1 for AV1; x264 for H.264. A proxy takes the first that
/// opens: NVENC allows a few sessions at a time.
pub fn encoders(choice: ProxyCodec, available: &Availability) -> Vec<ProxyEncoder> {
    let mut out = Vec::new();
    if choice != ProxyCodec::H264 {
        for (name, kind, pixel) in [
            ("av1_nvenc", EncoderChoice::Nvenc, format::Pixel::YUV420P),
            ("av1_vaapi", EncoderChoice::Vaapi, format::Pixel::NV12),
        ] {
            if let Some(status) = available.find(name, pixel) {
                out.push(ProxyEncoder {
                    name: name.into(),
                    codec: "av1",
                    kind,
                    device: status.device.clone(),
                });
            }
        }
    }
    let software = match choice {
        ProxyCodec::Av1 => ProxyEncoder::software("libsvtav1", "av1"),
        ProxyCodec::Auto | ProxyCodec::H264 => ProxyEncoder::software("libx264", "h264"),
    };
    if encoder::find_by_name(&software.name).is_some() {
        out.push(software);
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
}

/// Encodes `input`'s pictures to `dest` at `width`×`height` (even), with the
/// first of `encoders` that opens. Each frame is reported to this thread's
/// [`crate::progress`] guard, out of `total` (the source's frame count, when
/// known); this thread's [`crate::cancel`] flag stops it, leaving no file.
pub fn write(
    input: &Input,
    width: u32,
    height: u32,
    dest: &Path,
    total: Option<u64>,
    encoders: &[ProxyEncoder],
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
    let mut reader = VideoReader::open_input(input)?;
    let src_tb = reader.time_base();
    let enc_tb = encode_time_base(src_tb);
    let rotation = reader.rotation();
    let rate = reader.frame_rate();
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
    let mut errors = Vec::new();
    for enc in encoders {
        let result = encode_into(
            &mut reader,
            enc,
            (width, height, rotation),
            (src_tb, enc_tb, rate),
            &tmp,
            &shown,
        );
        match result {
            Ok(frames) => {
                std::fs::rename(&tmp, dest)
                    .map_err(|e| Error::Invalid(format!("replacing {}: {e}", dest.display())))?;
                return Ok(WrittenProxy {
                    path: dest.to_path_buf(),
                    width,
                    height,
                    frames,
                    encoder: enc.clone(),
                });
            }
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
            crate::export::Progress {
                frames: frames as i64,
                total: self.total.max(frames as i64),
                elapsed: self.started.elapsed(),
            },
        );
    }
}

/// Why [`encode_into`] failed: the encoder wouldn't open (another may), or
/// encoding failed.
enum Failed {
    Opening(Error),
    Encoding(Error),
}

/// Opens `enc` for `width`×`height` frames timed in `enc_tb`; for VAAPI,
/// with the context that uploads them.
fn open(
    enc: &ProxyEncoder,
    (width, height): (u32, u32),
    enc_tb: ffmpeg::Rational,
    rate: Option<ffmpeg::Rational>,
    global: bool,
) -> Result<(encoder::Video, ffmpeg::Codec, Option<Vaapi>)> {
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
    // Closed GOPs: every keyframe decodes on its own, so a seek can start
    // there (SVT-AV1 otherwise makes forward keyframes nothing seeks to).
    let mut flags = codec::Flags::CLOSED_GOP;
    if global {
        flags |= codec::Flags::GLOBAL_HEADER;
    }
    ctx.set_flags(flags);
    crate::color::tag(unsafe { ctx.as_mut_ptr() }, None);
    let vaapi = match (&enc.device, enc.kind) {
        (Some(node), EncoderChoice::Vaapi) => {
            let v = Vaapi::new(node, enc.pixel(), width, height)?;
            ctx.set_format(format::Pixel::VAAPI);
            let frames = unsafe { ffi::av_buffer_ref(v.frames) };
            if frames.is_null() {
                return Err(Error::Invalid("no memory for the VAAPI frames".into()));
            }
            unsafe { (*ctx.as_mut_ptr()).hw_frames_ctx = frames };
            Some(v)
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
    Ok((opened, codec, vaapi))
}

fn encode_into(
    reader: &mut VideoReader,
    enc: &ProxyEncoder,
    (width, height, rotation): (u32, u32, u16),
    (src_tb, enc_tb, rate): (ffmpeg::Rational, ffmpeg::Rational, Option<ffmpeg::Rational>),
    tmp: &Path,
    shown: &Shown<'_>,
) -> std::result::Result<u64, Failed> {
    let mut octx = format::output_as(tmp, "matroska")
        .ctx(|| format!("creating {}", tmp.display()))
        .map_err(Failed::Encoding)?;
    let global = octx.format().flags().contains(format::Flags::GLOBAL_HEADER);
    let (mut encoder, codec, vaapi) =
        open(enc, (width, height), enc_tb, rate, global).map_err(Failed::Opening)?;
    let mut encoding = || -> Result<u64> {
        let mut ost = octx
            .add_stream(Some(codec))
            .ctx(|| "adding the proxy stream".into())?;
        ost.set_parameters(&encoder);
        ost.set_time_base(enc_tb);
        octx.write_header()
            .ctx(|| "writing the proxy header".into())?;

        let mut normalizer: Option<Normalizer> = None;
        let mut durations: HashMap<i64, i64> = HashMap::new();
        let mut pending: Option<(frame::Video, i64)> = None;
        let mut last_dur = 1i64;
        let mut frames = 0u64;
        let mut encode_one = |durations: &mut HashMap<i64, i64>,
                              mut picture: frame::Video,
                              pts: i64,
                              dur: i64|
         -> Result<()> {
            picture.set_pts(Some(pts));
            // The source's picture type would force it on the encoder.
            picture.set_kind(ffmpeg::picture::Type::None);
            unsafe { (*picture.as_mut_ptr()).duration = dur };
            let mut picture = match &vaapi {
                Some(v) => v.upload(&picture)?,
                None => picture,
            };
            picture.set_pts(Some(pts));
            durations.insert(pts, dur);
            encoder
                .send_frame(&picture)
                .ctx(|| "encoding a proxy frame".into())?;
            drain(&mut encoder, &mut octx, enc_tb, durations)
        };
        while let Some(src) = reader.next_frame()? {
            let Some(raw) = src.pts() else {
                return Err(Error::Invalid(
                    "a source frame has no timestamp to copy onto the proxy".into(),
                ));
            };
            let mut pts = scale_pts(raw, src_tb, enc_tb);
            if let Some((_, prev)) = &pending
                && pts <= *prev
            {
                pts = prev.saturating_add(1);
            }
            let picture = scale(
                &mut normalizer,
                &src,
                rotation,
                (width, height),
                enc.pixel(),
            )?;
            if let Some((prev, prev_pts)) = pending.take() {
                let dur = (pts - prev_pts).max(1);
                last_dur = dur;
                encode_one(&mut durations, prev, prev_pts, dur)?;
                frames += 1;
                shown.frame(frames);
            }
            pending = Some((picture, pts));
        }
        if let Some((prev, prev_pts)) = pending.take() {
            encode_one(&mut durations, prev, prev_pts, last_dur)?;
            frames += 1;
            shown.frame(frames);
        }
        if frames == 0 {
            return Err(Error::Invalid(format!(
                "{}: no pictures to proxy",
                reader.path().display()
            )));
        }
        encoder
            .send_eof()
            .ctx(|| "flushing the proxy encoder".into())?;
        drain(&mut encoder, &mut octx, enc_tb, &mut durations)?;
        octx.write_trailer().ctx(|| "finishing the proxy".into())?;
        Ok(frames)
    };
    encoding().map_err(Failed::Encoding)
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
