//! The real [`Env`]: media facts from the FFmpeg libraries via vidcut-av.

use std::path::Path;

use vidcut_core::model::{AudioInfo, MediaInfo, VideoInfo};
use vidcut_core::time::Rational;
use vidcut_core::{Env, Error, ProbedMedia, Result};

pub struct AvEnv;

impl Env for AvEnv {
    fn probe(&self, path: &Path) -> Result<ProbedMedia> {
        let canonical = path
            .canonicalize()
            .map_err(|e| Error::media(format!("{}: {e}", path.display())))?;
        let meta = std::fs::metadata(&canonical)
            .map_err(|e| Error::media(format!("{}: {e}", path.display())))?;
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as i64);
        let info = vidcut_av::probe::probe(&canonical).map_err(|e| Error::media(e.to_string()))?;
        Ok(ProbedMedia {
            path: canonical,
            size: meta.len(),
            mtime_ns,
            info: convert(&info)?,
        })
    }
}

/// A stream's frame rate as a project-usable rational.
///
/// `r` (`r_frame_rate`, the rate all timestamps fit) is exact for constant
/// frame rate video, while the average can be skewed by container timestamp
/// rounding (Matroska stores milliseconds: 30 fps averages to 1000/33). So: a
/// standard `r` with an average within 2% wins; otherwise the average snapped
/// to a standard rate within 1% (variable frame rate around a standard rate);
/// otherwise the exact average.
fn frame_rate(avg: (i32, i32), r: (i32, i32)) -> Option<Rational> {
    let rate = |(n, d): (i32, i32)| {
        (n > 0 && d > 0)
            .then(|| Rational::new(i64::from(n), i64::from(d)).ok())
            .flatten()
    };
    let (avg, r) = (rate(avg), rate(r));
    if let (Some(r), Some(avg)) = (r, avg)
        && let Some(standard) = Rational::snap_standard(r.as_f64())
        && (avg.as_f64() / standard.as_f64() - 1.0).abs() < 0.02
    {
        return Some(standard);
    }
    let avg = avg.or(r)?;
    Some(
        crate::env::snap_within(avg.as_f64(), 0.01)
            .or_else(|| Rational::snap_standard(avg.as_f64()))
            .unwrap_or(avg),
    )
}

/// The standard rate within `tolerance` (relative) of `rate`.
fn snap_within(rate: f64, tolerance: f64) -> Option<Rational> {
    vidcut_core::time::STANDARD_RATES
        .iter()
        .filter_map(|&(n, d)| Rational::new(n, d).ok())
        .filter(|r| (rate / r.as_f64() - 1.0).abs() < tolerance)
        .min_by(|a, b| {
            (rate - a.as_f64())
                .abs()
                .total_cmp(&(rate - b.as_f64()).abs())
        })
}

fn convert(info: &vidcut_av::probe::MediaInfo) -> Result<MediaInfo> {
    let video = match &info.video {
        Some(v) => {
            let fps = frame_rate(v.avg_frame_rate, v.r_frame_rate).ok_or_else(|| {
                Error::media(format!("{}: unknown frame rate", info.path.display()))
            })?;
            let r = (v.r_frame_rate.0 > 0 && v.r_frame_rate.1 > 0)
                .then(|| f64::from(v.r_frame_rate.0) / f64::from(v.r_frame_rate.1));
            let avg = (v.avg_frame_rate.0 > 0 && v.avg_frame_rate.1 > 0)
                .then(|| f64::from(v.avg_frame_rate.0) / f64::from(v.avg_frame_rate.1));
            Some(VideoInfo {
                codec: v.codec.clone(),
                width: v.width,
                height: v.height,
                sar: [v.sar.0, v.sar.1],
                pix_fmt: v.pix_fmt.clone(),
                fps,
                // r_frame_rate is the rate all timestamps fit; well above the
                // average it signals variable frame rate.
                vfr: matches!((r, avg), (Some(r), Some(avg)) if r > avg * 1.05),
                first_pts: v.first_frame_time.unwrap_or(0.0),
                duration: v.duration_secs.or(info.duration).unwrap_or(0.0).max(0.0),
            })
        }
        None => None,
    };
    let audio = info
        .audio
        .iter()
        .enumerate()
        .map(|(i, a)| AudioInfo {
            index: i as u32,
            codec: a.codec.clone(),
            sample_rate: a.sample_rate,
            channels: a.channels,
            first_pts: a.start_secs.unwrap_or(0.0),
            duration: a.duration_secs.or(info.duration).unwrap_or(0.0).max(0.0),
        })
        .collect();
    Ok(MediaInfo {
        format: info.format.clone(),
        duration: info.duration.unwrap_or(0.0),
        start_time: info.start_time.unwrap_or(0.0),
        video,
        audio,
    })
}

#[cfg(test)]
mod tests {
    use super::frame_rate;

    #[test]
    fn frame_rates_from_stream_fields() {
        let f = |avg, r| frame_rate(avg, r).map(|r| r.to_string());
        assert_eq!(
            f((1000, 33), (30, 1)).as_deref(),
            Some("30"),
            "Matroska ms rounding"
        );
        assert_eq!(
            f((30000, 1001), (30000, 1001)).as_deref(),
            Some("30000/1001")
        );
        // VFR: the average (29.83) snaps to the nearest standard rate.
        assert_eq!(f((5400, 181), (60, 1)).as_deref(), Some("30000/1001"));
        assert_eq!(
            f((24000, 1001), (24000, 1001)).as_deref(),
            Some("24000/1001")
        );
        assert_eq!(
            f((15, 1), (15, 1)).as_deref(),
            Some("15"),
            "non-standard rates stay exact"
        );
        assert_eq!(f((0, 0), (25, 1)).as_deref(), Some("25"));
    }
}
