//! Media probing: container and stream facts needed for editing and export.

use std::path::{Path, PathBuf};

use crate::decode::Input;
use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, media};

/// A rational number as (numerator, denominator), e.g. a time base or frame rate.
pub type Ratio = (i32, i32);

fn ratio(r: ffmpeg::Rational) -> Ratio {
    (r.numerator(), r.denominator())
}

fn opt_ts(ts: i64) -> Option<i64> {
    (ts != ffmpeg::ffi::AV_NOPTS_VALUE).then_some(ts)
}

/// Facts about a media file.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaInfo {
    pub path: PathBuf,
    /// Demuxer short name(s), e.g. `mov,mp4,m4a,3gp,3g2,mj2`.
    pub format: String,
    /// Container duration in seconds, if known.
    pub duration: Option<f64>,
    /// Container start time in seconds (`fs` in the time map), if known.
    pub start_time: Option<f64>,
    /// The primary ("best") video stream, if any.
    pub video: Option<VideoStreamInfo>,
    /// All audio streams, in file order.
    pub audio: Vec<AudioStreamInfo>,
    /// The start timecode the file records (`HH:MM:SS:FF`, `;` before the
    /// frames for drop-frame): a QuickTime/MP4 `tmcd` track's, MXF's, or a
    /// `timecode` tag's. Counted in the video's frames.
    pub timecode: Option<String>,
    /// A broadcast WAV's time reference: samples since midnight where the
    /// recording starts (the `bext` chunk).
    pub time_reference: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VideoStreamInfo {
    pub index: usize,
    pub codec: String,
    pub width: u32,
    pub height: u32,
    /// Sample (pixel) aspect ratio; (0, 1) when unknown.
    pub sar: Ratio,
    pub pix_fmt: String,
    pub time_base: Ratio,
    pub avg_frame_rate: Ratio,
    pub r_frame_rate: Ratio,
    /// Frame count from the container, if recorded.
    pub frames: Option<i64>,
    /// Stream start timestamp in `time_base` units, if known.
    pub start_pts: Option<i64>,
    /// Stream duration in `time_base` units, if known.
    pub duration_pts: Option<i64>,
    /// Raw timestamp of the first decodable frame, seconds (`o` in the time map).
    pub first_frame_time: Option<f64>,
    /// Seconds from the first frame to the end of the stream.
    pub duration_secs: Option<f64>,
    /// Clockwise rotation (0, 90, 180 or 270) that shows the pictures upright.
    pub rotation: u16,
    /// Interlaced, with the top field shown first (`Some(true)`) or the
    /// bottom one; `None` when progressive.
    pub top_field_first: Option<bool>,
    /// Transfer characteristic, FFmpeg's name: `bt709`, `smpte2084` (PQ),
    /// `arib-std-b67` (HLG), `unknown`, ….
    pub transfer: String,
    /// A single picture (an image file), not a video.
    pub still: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioStreamInfo {
    pub index: usize,
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub time_base: Ratio,
    pub start_pts: Option<i64>,
    /// Stream start in seconds, if known.
    pub start_secs: Option<f64>,
    /// Samples the encoder put before the content (AAC priming, Opus
    /// pre-skip) that decoders still output: the stream's `initial_padding`
    /// less what the demuxer has the decoder skip (see [`start_skips`]).
    pub padding: u32,
    /// Where the content starts, seconds: the start plus the padding,
    /// rounded to the stream's time base. Containers that keep the padding
    /// give it negative timestamps (Matroska: the first packet at −21 ms).
    pub first_secs: Option<f64>,
    /// Stream duration in seconds, if known (padding included).
    pub duration_secs: Option<f64>,
}

/// Stream duration in seconds: the container's value, else Matroska's
/// `DURATION` tag (`HH:MM:SS.nnnnnnnnn`), else `None`.
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

/// Whether FFmpeg reads files of demuxer `format` as images: image2, and
/// the `*_pipe` demuxers it picks for one image file (`png_pipe`, …).
fn image_format(format: &str) -> bool {
    format == "image2" || format.ends_with("_pipe")
}

/// Probes `path`, including the first decodable video frame (one frame is
/// decoded) and stream durations. An image file is a still.
pub fn probe(path: &Path) -> Result<MediaInfo> {
    probe_input(&Input::File(path.to_path_buf()))
}

/// Probes an image sequence: the files `pattern` names from the number
/// `first`, shown at `rate` (numerator, denominator). `count` is how many
/// the caller expects.
///
/// The frame count and duration are the contiguous run image2 opened
/// (its stream duration, in frames), and at most `count` when that run is
/// longer. A missing file ends the run, so a short prefix is not reported
/// as the requested count.
pub fn probe_sequence(
    pattern: &Path,
    first: u64,
    count: u64,
    rate: (i32, i32),
) -> Result<MediaInfo> {
    let mut info = probe_input(&Input::Sequence {
        pattern: pattern.to_path_buf(),
        first,
        rate,
    })?;
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

/// Frames in the contiguous run image2 opened.
///
/// With a time base of one frame, the stream duration is that count:
/// `last − first + 1` from image2's search, which stops at the first
/// missing file.
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

fn probe_input(source: &Input) -> Result<MediaInfo> {
    let path = source.path();
    let mut input = source.open()?;
    let skips = start_skips(&mut input);
    let still = matches!(source, Input::File(_)) && image_format(input.format().name());
    let (duration, start_time) = unsafe {
        let ctx = input.as_ptr();
        let to_secs = |v: i64| opt_ts(v).map(|v| v as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE));
        (to_secs((*ctx).duration), to_secs((*ctx).start_time))
    };

    let best_video = input.streams().best(media::Type::Video).map(|s| s.index());
    let mut video = None;
    let mut audio = Vec::new();
    for stream in input.streams() {
        let params = stream.parameters();
        let codec = params.id().name().to_string();
        match params.medium() {
            media::Type::Video if Some(stream.index()) == best_video => {
                let raw = unsafe { &*params.as_ptr() };
                // AVCodecParameters stores the pixel format as a plain int; an
                // (unopened) decoder context converts it to the enum safely.
                let pix_fmt = ffmpeg::codec::context::Context::from_parameters(params.clone())
                    .and_then(|c| c.decoder().video())
                    .map(|d| d.format())
                    .ctx(|| format!("reading video parameters of {}", path.display()))?;
                let tb = stream.time_base();
                let secs =
                    |t: i64| t as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator());
                let start = opt_ts(stream.start_time()).map(secs);
                let reader = crate::decode::VideoReader::open_input(source).ok();
                let first_frame_time = reader.as_ref().and_then(|r| r.first_pts()).map(secs);
                let first = reader.as_ref().and_then(|r| r.first_props());
                // Field order: the stream's, else the first frame's flags.
                use ffmpeg::ffi::AVFieldOrder as F;
                let top_field_first = match raw.field_order {
                    F::AV_FIELD_TT | F::AV_FIELD_BT => Some(true),
                    F::AV_FIELD_BB | F::AV_FIELD_TB => Some(false),
                    _ => first.and_then(|f| f.top_field_first),
                };
                // Transfer: the stream's, else the first frame's.
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
                // Duration from the first frame: the stream's end (start + its
                // duration) minus the first frame's time; else the container's.
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
                });
            }
            media::Type::Audio => {
                let raw = unsafe { &*params.as_ptr() };
                let tb = stream.time_base();
                let start_pts = opt_ts(stream.start_time());
                // The stream's start already counts the samples skipped.
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
        format: input.format().name().to_string(),
        duration,
        start_time,
        video,
        audio,
        timecode,
        time_reference,
    })
}

/// The samples the demuxer has the decoder skip at the start of each audio
/// stream with priming (`initial_padding`), as (stream index, samples):
/// the skip-samples side data of the stream's first packet. Matroska keeps
/// AAC and Opus priming as a codec delay, which FFmpeg 8.1 and later skip
/// this way and 8.0 leaves in the decoded audio. Reads packets only when a
/// stream has priming, and at most a few hundred.
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

/// The start timecode and broadcast WAV time reference an opened file
/// records (see [`MediaInfo::timecode`]). The video stream's tag wins (the
/// MOV demuxer copies its `tmcd` track's there), then any stream's (the
/// `tmcd` track itself), then the container's (MXF).
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

/// Only the start timecode and time reference of `path`, from the
/// container's header (nothing is decoded): for media probed before vidcut
/// kept them.
pub fn clock(path: &Path) -> Result<(Option<String>, Option<u64>)> {
    crate::init();
    let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| Error::Invalid(format!("{}: a NUL in the name", path.display())))?;
    // The header holds the tags (a MOV's timecode track is read with it);
    // finding stream info would decode frames.
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
    // Without stream info the best video stream is the first one.
    let video = input
        .streams()
        .find(|s| s.parameters().medium() == media::Type::Video)
        .map(|s| s.index());
    Ok(clock_tags(&input, video))
}
