//! Media probing: container and stream facts needed for editing and export.

use std::path::{Path, PathBuf};

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
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioStreamInfo {
    pub index: usize,
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub time_base: Ratio,
    pub start_pts: Option<i64>,
}

/// Probes `path`.
pub fn probe(path: &Path) -> Result<MediaInfo> {
    crate::init();
    let input = ffmpeg::format::input(path).ctx(|| format!("opening {}", path.display()))?;
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
                });
            }
            media::Type::Audio => {
                let raw = unsafe { &*params.as_ptr() };
                audio.push(AudioStreamInfo {
                    index: stream.index(),
                    codec,
                    sample_rate: raw.sample_rate as u32,
                    channels: raw.ch_layout.nb_channels as u16,
                    time_base: ratio(stream.time_base()),
                    start_pts: opt_ts(stream.start_time()),
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
    Ok(MediaInfo {
        path: path.to_path_buf(),
        format: input.format().name().to_string(),
        duration,
        start_time,
        video,
        audio,
    })
}
