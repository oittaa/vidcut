use std::path::{Path, PathBuf};

use crate::decode::Input;
use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, media};

pub type Ratio = (i32, i32);

fn ratio(r: ffmpeg::Rational) -> Ratio {
    (r.numerator(), r.denominator())
}

fn opt_ts(ts: i64) -> Option<i64> {
    (ts != ffmpeg::ffi::AV_NOPTS_VALUE).then_some(ts)
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaInfo {
    pub path: PathBuf,
    pub format: String,
    pub duration: Option<f64>,
    pub start_time: Option<f64>,
    pub video: Option<VideoStreamInfo>,
    pub audio: Vec<AudioStreamInfo>,
    pub timecode: Option<String>,
    pub time_reference: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VideoStreamInfo {
    pub index: usize,
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub sar: Ratio,
    pub pix_fmt: String,
    pub time_base: Ratio,
    pub avg_frame_rate: Ratio,
    pub r_frame_rate: Ratio,
    pub frames: Option<i64>,
    pub start_pts: Option<i64>,
    pub duration_pts: Option<i64>,
    pub first_frame_time: Option<f64>,
    pub duration_secs: Option<f64>,
    pub rotation: u16,
    pub top_field_first: Option<bool>,
    pub transfer: String,
    pub still: bool,
    pub hdr: crate::hdr::Metadata,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioStreamInfo {
    pub index: usize,
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub time_base: Ratio,
    pub start_pts: Option<i64>,
    pub start_secs: Option<f64>,
    pub padding: u32,
    pub first_secs: Option<f64>,
    pub duration_secs: Option<f64>,
}

fn stream_duration(stream: &ffmpeg::format::stream::Stream) -> Option<f64> {
    let tb = stream.time_base();
    if let Some(d) = opt_ts(stream.duration()).filter(|d| *d > 0) {
        return Some(d as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator()));
    }
    let tag = stream.metadata().get("DURATION").map(str::to_string)?;
    let mut parts = tag.split(':');
    let (h, m, s) = (
        parts.next()?.parse::<f64>().ok()?,
        parts.next()?.parse::<f64>().ok()?,
        parts.next()?.parse::<f64>().ok()?,
    );
    Some(h * 3600.0 + m * 60.0 + s)
}

fn image_format(format: &str) -> bool {
    format == "image2" || format.ends_with("_pipe")
}

fn animation_format(format: &str) -> bool {
    matches!(format, "gif" | "webp_anim")
}

fn pictures(input: &mut ffmpeg::format::context::Input, all: bool) -> (usize, Option<f64>) {
    let Some((video, tb)) = input
        .streams()
        .best(media::Type::Video)
        .map(|s| (s.index(), s.time_base()))
    else {
        return (0, None);
    };
    let (mut count, mut end) = (0, None);
    for (stream, packet) in input.packets() {
        if stream.index() != video {
            continue;
        }
        count += 1;
        if let Some(pts) = packet.pts() {
            end = end.max(Some(pts + packet.duration().max(0)));
        }
        if !all && count == 2 {
            break;
        }
    }
    let secs = end.map(|e| e as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator()));
    (count, secs)
}

pub fn pixel_depth(name: &str) -> u8 {
    use std::str::FromStr;
    ffmpeg::format::Pixel::from_str(name)
        .ok()
        .and_then(|p| p.descriptor())
        .map_or(8, |d| unsafe { (*d.as_ptr()).comp[0].depth } as u8)
}

fn c_name(name: *const std::ffi::c_char) -> Option<String> {
    // SAFETY: FFmpeg's static, NUL-terminated names, or null.
    (!name.is_null()).then(|| {
        unsafe { std::ffi::CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned()
    })
}

fn tags(dict: ffmpeg::DictionaryRef<'_>) -> std::collections::BTreeMap<String, String> {
    dict.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn rate(bits: i64) -> Option<u64> {
    u64::try_from(bits).ok().filter(|&b| b > 0)
}

fn chroma(d: &ffmpeg::ffi::AVPixFmtDescriptor) -> Option<String> {
    if d.nb_components < 3 || d.flags & ffmpeg::ffi::AV_PIX_FMT_FLAG_RGB as u64 != 0 {
        return None;
    }
    Some(
        match (d.log2_chroma_w, d.log2_chroma_h) {
            (0, 0) => "4:4:4",
            (1, 0) => "4:2:2",
            (1, 1) => "4:2:0",
            (2, 0) => "4:1:1",
            (2, 2) => "4:1:0",
            _ => return None,
        }
        .to_string(),
    )
}

pub fn pixel_chroma(name: &str) -> Option<String> {
    use std::str::FromStr;
    let d = ffmpeg::format::Pixel::from_str(name).ok()?.descriptor()?;
    // SAFETY: FFmpeg's static descriptor.
    chroma(unsafe { &*d.as_ptr() })
}

pub fn details(path: &Path) -> Result<vidcut_core::model::MediaDetails> {
    use ffmpeg::ffi;
    let input = Input::File(path.to_path_buf()).open()?;
    // SAFETY: an open input's context.
    let bit_rate = unsafe { (*input.as_ptr()).bit_rate };
    let mut streams = Vec::new();
    for stream in input.streams() {
        let params = stream.parameters();
        // SAFETY: the stream's own parameters.
        let raw = unsafe { &*params.as_ptr() };
        let id = params.id();
        // SAFETY: a lookup of the codec's static descriptor.
        let descriptor = unsafe { ffi::avcodec_descriptor_get(id.into()) };
        let tags = tags(stream.metadata());
        let mut d = vidcut_core::model::StreamDetails {
            index: stream.index() as u32,
            kind: match params.medium() {
                media::Type::Video => "video",
                media::Type::Audio => "audio",
                media::Type::Subtitle => "subtitle",
                media::Type::Attachment => "attachment",
                _ => "data",
            }
            .to_string(),
            codec: id.name().to_string(),
            // SAFETY: the descriptor is FFmpeg's static one, or null.
            codec_name: (!descriptor.is_null())
                .then(|| c_name(unsafe { (*descriptor).long_name }))
                .flatten()
                .unwrap_or_default(),
            // SAFETY: a lookup of a static profile name.
            profile: c_name(unsafe { ffi::avcodec_profile_name(id.into(), raw.profile) }),
            bit_rate: rate(raw.bit_rate),
            language: tags.get("language").cloned(),
            tags,
            ..Default::default()
        };
        match params.medium() {
            media::Type::Video => {
                let pixel = ffmpeg::codec::context::Context::from_parameters(params.clone())
                    .and_then(|c| c.decoder().video())
                    .ok()
                    .and_then(|v| v.format().descriptor());
                if let Some(pixel) = pixel {
                    // SAFETY: FFmpeg's static descriptor.
                    let pixel = unsafe { &*pixel.as_ptr() };
                    d.pix_fmt = c_name(pixel.name);
                    d.bits = Some(u32::from(pixel.comp[0].depth as u8));
                    d.chroma = chroma(pixel);
                }
                // SAFETY: lookups of static color names.
                unsafe {
                    d.primaries = c_name(ffi::av_color_primaries_name(raw.color_primaries));
                    d.transfer = c_name(ffi::av_color_transfer_name(raw.color_trc));
                    d.matrix = c_name(ffi::av_color_space_name(raw.color_space));
                    d.range = c_name(ffi::av_color_range_name(raw.color_range));
                }
            }
            media::Type::Audio => {
                d.sample_rate = u32::try_from(raw.sample_rate).ok().filter(|&r| r > 0);
                d.channels = u16::try_from(raw.ch_layout.nb_channels)
                    .ok()
                    .filter(|&c| c > 0);
                let mut buf = [0 as std::ffi::c_char; 64];
                // SAFETY: describes the stream's layout into a buffer of its length.
                let n = unsafe {
                    ffi::av_channel_layout_describe(&raw.ch_layout, buf.as_mut_ptr(), buf.len())
                };
                if n > 0 {
                    d.layout = c_name(buf.as_ptr());
                }
                d.sample_format = ffmpeg::codec::context::Context::from_parameters(params.clone())
                    .and_then(|c| c.decoder().audio())
                    .ok()
                    .map(|a| a.format().name().to_string());
            }
            _ => {}
        }
        streams.push(d);
    }
    Ok(vidcut_core::model::MediaDetails {
        format: input.format().description().to_string(),
        bit_rate: rate(bit_rate),
        tags: tags(input.metadata()),
        streams,
    })
}

pub fn probe(path: &Path, decoder: Option<&crate::hw::GpuDecoder>) -> Result<MediaInfo> {
    probe_input(&Input::File(path.to_path_buf()), decoder)
}

pub fn probe_sequence(
    pattern: &Path,
    first: u64,
    count: u64,
    rate: (i32, i32),
) -> Result<MediaInfo> {
    let source = Input::Sequence {
        pattern: pattern.to_path_buf(),
        first,
        rate,
    };
    let mut info = probe_input(&source, None)?;
    let opened = sequence_frames(&info, rate).ok_or_else(|| {
        Error::Invalid(format!(
            "{}: image2 did not report how many frames it opened from {first}",
            pattern.display()
        ))
    })?;
    let frames = opened.min(count);
    let secs = frames as f64 * f64::from(rate.1) / f64::from(rate.0);
    info.duration = Some(secs);
    if let Some(v) = info.video.as_mut() {
        v.frames = Some(frames as i64);
        v.duration_secs = Some(secs);
        if v.time_base == (rate.1, rate.0) {
            v.duration_pts = Some(frames as i64);
        }
        v.avg_frame_rate = rate;
        v.r_frame_rate = rate;
    }
    Ok(info)
}

fn sequence_frames(info: &MediaInfo, rate: (i32, i32)) -> Option<u64> {
    let v = info.video.as_ref()?;
    if rate.0 <= 0 || rate.1 <= 0 {
        return None;
    }
    if let Some(dur) = v.duration_pts.filter(|d| *d > 0) {
        let tb = v.time_base;
        if tb == (rate.1, rate.0) {
            return Some(dur as u64);
        }
        if tb.0 > 0 && tb.1 > 0 {
            let frames = dur as f64 * f64::from(tb.0) / f64::from(tb.1) * f64::from(rate.0)
                / f64::from(rate.1);
            if frames >= 1.0 {
                return Some(frames.round() as u64);
            }
        }
    }
    v.duration_secs
        .filter(|s| *s > 0.0)
        .or(info.duration.filter(|s| *s > 0.0))
        .map(|s| (s * f64::from(rate.0) / f64::from(rate.1)).round() as u64)
        .filter(|n| *n >= 1)
}

fn probe_input(source: &Input, decoder: Option<&crate::hw::GpuDecoder>) -> Result<MediaInfo> {
    let path = source.path();
    let mut input = source.open()?;
    let skips = start_skips(&mut input);
    let format = input.format().name().to_string();
    let (mut duration, start_time) = unsafe {
        let ctx = input.as_ptr();
        let to_secs = |v: i64| opt_ts(v).map(|v| v as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE));
        (to_secs((*ctx).duration), to_secs((*ctx).start_time))
    };
    let file = matches!(source, Input::File(_));
    let (count, end) = match file && animation_format(&format) {
        true => pictures(&mut input, duration.is_none()),
        false => (0, None),
    };
    let still = file && (image_format(&format) || count == 1);
    if duration.is_none() && !still {
        duration = end.map(|e| e - start_time.unwrap_or(0.0));
    }

    let best_video = input.streams().best(media::Type::Video).map(|s| s.index());
    let mut video = None;
    let mut audio = Vec::new();
    for stream in input.streams() {
        let params = stream.parameters();
        let codec = params.id().name().to_string();
        match params.medium() {
            media::Type::Video if Some(stream.index()) == best_video => {
                let raw = unsafe { &*params.as_ptr() };
                let pix_fmt = ffmpeg::codec::context::Context::from_parameters(params.clone())
                    .and_then(|c| c.decoder().video())
                    .map(|d| d.format())
                    .ctx(|| format!("reading video parameters of {}", path.display()))?;
                let tb = stream.time_base();
                let secs =
                    |t: i64| t as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator());
                let start = opt_ts(stream.start_time()).map(secs);
                let reader = match decoder.filter(|_| !still) {
                    Some(gpu) => crate::decode::VideoReader::open_gpu_or_cpu(source, gpu),
                    None => crate::decode::VideoReader::open_input(source),
                }
                .ok();
                let first_frame_time = reader.as_ref().and_then(|r| r.first_pts()).map(secs);
                let first = reader.as_ref().and_then(|r| r.first_props());
                use ffmpeg::ffi::AVFieldOrder as F;
                let top_field_first = match raw.field_order {
                    F::AV_FIELD_TT | F::AV_FIELD_BT => Some(true),
                    F::AV_FIELD_BB | F::AV_FIELD_TB => Some(false),
                    _ => first.and_then(|f| f.top_field_first),
                };
                let transfer = match raw.color_trc {
                    ffmpeg::ffi::AVColorTransferCharacteristic::AVCOL_TRC_UNSPECIFIED => {
                        first.map_or(raw.color_trc, |f| f.transfer)
                    }
                    t => t,
                };
                let transfer = unsafe {
                    let name = ffmpeg::ffi::av_color_transfer_name(transfer);
                    if name.is_null() {
                        "unknown".to_string()
                    } else {
                        std::ffi::CStr::from_ptr(name)
                            .to_string_lossy()
                            .into_owned()
                    }
                };
                let end = match (start, stream_duration(&stream)) {
                    (Some(s), Some(d)) => Some(s + d),
                    (None, Some(d)) => Some(d),
                    _ => duration.map(|d| d + start_time.unwrap_or(0.0)),
                };
                let duration_secs = match (end, first_frame_time.or(start)) {
                    (Some(e), Some(o)) => Some(e - o),
                    (Some(e), None) => Some(e),
                    _ => None,
                };
                video = Some(VideoStreamInfo {
                    index: stream.index(),
                    codec,
                    width: raw.width as u32,
                    height: raw.height as u32,
                    sar: (raw.sample_aspect_ratio.num, raw.sample_aspect_ratio.den),
                    pix_fmt: pix_fmt
                        .descriptor()
                        .map(|d| d.name().to_string())
                        .unwrap_or_default(),
                    time_base: ratio(stream.time_base()),
                    avg_frame_rate: ratio(stream.avg_frame_rate()),
                    r_frame_rate: ratio(stream.rate()),
                    frames: (stream.frames() > 0).then_some(stream.frames()),
                    start_pts: opt_ts(stream.start_time()),
                    duration_pts: opt_ts(stream.duration()).filter(|d| *d > 0),
                    first_frame_time,
                    duration_secs,
                    rotation: crate::decode::stream_rotation(&params),
                    top_field_first,
                    transfer,
                    still,
                    hdr: crate::hdr::Metadata {
                        dolby_vision: crate::hdr::dolby_vision(&params),
                        ..first.map(|f| f.hdr).unwrap_or_default()
                    },
                });
            }
            media::Type::Audio => {
                let raw = unsafe { &*params.as_ptr() };
                let tb = stream.time_base();
                let start_pts = opt_ts(stream.start_time());
                let skipped = skips
                    .iter()
                    .find_map(|&(i, n)| (i == stream.index()).then_some(n));
                let padding =
                    (raw.initial_padding.max(0) as u32).saturating_sub(skipped.unwrap_or(0));
                let tick = f64::from(tb.numerator()) / f64::from(tb.denominator());
                let first_secs = start_pts.map(|t| {
                    let rate = f64::from(raw.sample_rate.max(1));
                    let first = t as f64 * tick + f64::from(padding) / rate;
                    (first / tick).round() * tick
                });
                audio.push(AudioStreamInfo {
                    index: stream.index(),
                    codec,
                    sample_rate: raw.sample_rate as u32,
                    channels: raw.ch_layout.nb_channels as u16,
                    time_base: ratio(tb),
                    start_pts,
                    start_secs: start_pts.map(|t| t as f64 * tick),
                    padding,
                    first_secs,
                    duration_secs: stream_duration(&stream).or(duration),
                });
            }
            _ => {}
        }
    }
    if video.is_none() && audio.is_empty() {
        return Err(Error::Invalid(format!(
            "{}: no audio or video streams",
            path.display()
        )));
    }
    let (timecode, time_reference) = clock_tags(&input, best_video);
    Ok(MediaInfo {
        path: path.to_path_buf(),
        format,
        duration,
        start_time,
        video,
        audio,
        timecode,
        time_reference,
    })
}

fn start_skips(input: &mut ffmpeg::format::context::Input) -> Vec<(usize, u32)> {
    let mut want: Vec<usize> = input
        .streams()
        .filter(|s| {
            let params = s.parameters();
            params.medium() == media::Type::Audio
                && unsafe { (*params.as_ptr()).initial_padding } > 0
        })
        .map(|s| s.index())
        .collect();
    let mut skips = Vec::new();
    if want.is_empty() {
        return skips;
    }
    for (stream, packet) in input.packets().take(500) {
        let Some(at) = want.iter().position(|&i| i == stream.index()) else {
            continue;
        };
        want.swap_remove(at);
        let skip = packet
            .side_data()
            .find(|d| d.kind() == ffmpeg::packet::side_data::Type::SkipSamples)
            .and_then(|d| d.data().first_chunk::<4>().map(|b| u32::from_le_bytes(*b)))
            .unwrap_or(0);
        skips.push((stream.index(), skip));
        if want.is_empty() {
            break;
        }
    }
    skips
}

fn clock_tags(
    input: &ffmpeg::format::context::Input,
    video: Option<usize>,
) -> (Option<String>, Option<u64>) {
    let tag = |d: ffmpeg::DictionaryRef| {
        d.get("timecode")
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(String::from)
    };
    let timecode = video
        .and_then(|i| input.stream(i))
        .and_then(|s| tag(s.metadata()))
        .or_else(|| input.streams().find_map(|s| tag(s.metadata())))
        .or_else(|| tag(input.metadata()));
    let time_reference = input
        .metadata()
        .get("time_reference")
        .and_then(|v| v.trim().parse().ok());
    (timecode, time_reference)
}

pub fn clock(path: &Path) -> Result<(Option<String>, Option<u64>)> {
    crate::init();
    let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| Error::Invalid(format!("{}: a NUL in the name", path.display())))?;
    let input = unsafe {
        let mut ctx = std::ptr::null_mut();
        let ret = ffmpeg::ffi::avformat_open_input(
            &mut ctx,
            name.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
        );
        if ret < 0 {
            return Err(Error::Ffmpeg {
                context: format!("opening {}", path.display()),
                source: ffmpeg::Error::from(ret),
            });
        }
        ffmpeg::format::context::Input::wrap(ctx)
    };
    let video = input
        .streams()
        .find(|s| s.parameters().medium() == media::Type::Video)
        .map(|s| s.index());
    Ok(clock_tags(&input, video))
}

#[cfg(test)]
mod tests {
    #[test]
    fn pixel_depths_come_from_the_format() {
        crate::init();
        for (name, depth) in [
            ("yuv420p", 8),
            ("nv12", 8),
            ("rgb24", 8),
            ("yuv420p10le", 10),
            ("p010le", 10),
            ("yuv422p12le", 12),
            ("rgb48be", 16),
            ("no-such-format", 8),
        ] {
            assert_eq!(super::pixel_depth(name), depth, "{name}");
        }
    }

    #[test]
    fn a_four_two_zero_formats_chroma_reads_as_such() {
        crate::init();
        assert_eq!(super::pixel_chroma("yuv420p10le").as_deref(), Some("4:2:0"));
    }

    #[test]
    fn an_rgb_format_has_no_chroma_subsampling() {
        crate::init();
        assert_eq!(super::pixel_chroma("rgb24"), None);
    }
}
