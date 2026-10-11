//! The real [`Env`]: media facts from the FFmpeg libraries via vidcut-av.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use vidcut_core::analysis::{AudioLevels, Loudness, PictureStats};
use vidcut_core::model::{AudioInfo, FieldOrder, Hdr, Media, MediaInfo, VideoInfo};
use vidcut_core::render::RenderPlan;
use vidcut_core::time::Rational;
use vidcut_core::{Env, Error, ProbedMedia, Result};

use crate::paths;

pub struct AvEnv;

/// Audio levels are measured in 10 ms buckets.
const LEVELS_PER_SECOND: u32 = 100;

/// Measurements made in this process, by cache file.
type Memo<T> = LazyLock<Mutex<HashMap<PathBuf, Arc<T>>>>;
static LEVELS: Memo<AudioLevels> = LazyLock::new(Default::default);
static PICTURES: Memo<PictureStats> = LazyLock::new(Default::default);

/// A measurement of a media file: from this process's memory, from the disk
/// cache, or measured now and kept in both. Measuring decodes the whole
/// stream, so the live session measures on a worker first (docs/decisions/agents.md §3).
fn measured<T>(
    memo: &Memo<T>,
    file: PathBuf,
    measure: impl FnOnce() -> vidcut_av::error::Result<T>,
    to_bytes: impl Fn(&T) -> Vec<u8>,
    from_bytes: impl Fn(&[u8]) -> Option<T>,
) -> Result<Arc<T>> {
    let lock = || memo.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(v) = lock().get(&file) {
        return Ok(v.clone());
    }
    let value = match std::fs::read(&file).ok().and_then(|b| from_bytes(&b)) {
        Some(v) => v,
        None => {
            let v = measure().map_err(av_error)?;
            if let Some(dir) = file.parent()
                && std::fs::create_dir_all(dir).is_ok()
            {
                let part = file.with_extension("part");
                if std::fs::write(&part, to_bytes(&v)).is_ok() {
                    let _ = std::fs::rename(&part, &file);
                }
            }
            v
        }
    };
    let value = Arc::new(value);
    lock().insert(file, value.clone());
    Ok(value)
}

/// How `m`'s pictures are read: its file, or its image sequence.
pub fn input_of(m: &Media) -> vidcut_av::decode::Input {
    use vidcut_av::decode::Input;
    match (&m.sequence, &m.info.video) {
        (Some(s), Some(v)) => Input::Sequence {
            pattern: m.path.parent().unwrap_or(Path::new("")).join(&s.pattern),
            first: s.first,
            rate: (v.fps.num() as i32, v.fps.den() as i32),
        },
        _ => Input::File(m.path.clone()),
    }
}

impl Env for AvEnv {
    fn probe(&self, path: &Path) -> Result<ProbedMedia> {
        let canonical = path
            .canonicalize()
            .map_err(|e| Error::media(format!("{}: {e}", path.display())))?;
        let meta = std::fs::metadata(&canonical)
            .map_err(|e| Error::media(format!("{}: {e}", path.display())))?;
        // Media are files. A device or a pipe (which an imported timeline
        // could name) might never end, or block the probe for good.
        if !meta.is_file() {
            return Err(Error::media(format!(
                "{}: not a regular file",
                path.display()
            )));
        }
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
            sequence: None,
        })
    }

    fn probe_sequence(&self, path: &Path, rate: Rational) -> Result<ProbedMedia> {
        let canonical = path
            .canonicalize()
            .map_err(|e| Error::media(format!("{}: {e}", path.display())))?;
        let sequence = vidcut_core::sequence::find(&canonical)?;
        let dir = canonical.parent().unwrap_or(Path::new("/"));
        let first = sequence.file(dir, 0);
        let mut probed = self.probe(&first)?;
        let info = vidcut_av::probe::probe_sequence(
            &dir.join(&sequence.pattern),
            sequence.first,
            sequence.count,
            (rate.num() as i32, rate.den() as i32),
        )
        .map_err(|e| Error::media(e.to_string()))?;
        probed.info = convert(&info)?;
        if let Some(v) = probed.info.video.as_mut() {
            // The rate asked for, exactly.
            (v.fps, v.vfr) = (rate, false);
        }
        probed.sequence = Some(sequence);
        Ok(probed)
    }

    fn audio_levels(&self, media: &Media, stream: u32) -> Result<Arc<AudioLevels>> {
        measured(
            &LEVELS,
            paths::media_cache("analysis", media, &format!("levels{stream}")),
            || vidcut_av::analysis::audio_levels(&media.path, stream, LEVELS_PER_SECOND),
            AudioLevels::to_bytes,
            AudioLevels::from_bytes,
        )
    }

    fn picture_stats(&self, media: &Media) -> Result<Arc<PictureStats>> {
        measured(
            &PICTURES,
            paths::media_cache("analysis", media, "pictures"),
            || vidcut_av::analysis::picture_stats(&input_of(media)),
            PictureStats::to_bytes,
            PictureStats::from_bytes,
        )
    }

    fn loudness(&self, plan: &RenderPlan) -> Result<Loudness> {
        vidcut_av::analysis::loudness(plan).map_err(av_error)
    }
}

/// A measurement's failure. Cancellation uses the same flag as an export, so
/// it is `CANCELLED` and not a media error.
fn av_error(e: vidcut_av::error::Error) -> Error {
    if vidcut_av::export::is_cancelled(&e) {
        Error::new(vidcut_core::ErrorCode::Cancelled, "cancelled")
    } else {
        Error::media(e.to_string())
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
                rotation: v.rotation,
                fields: v.top_field_first.map(|top| {
                    if top {
                        FieldOrder::Top
                    } else {
                        FieldOrder::Bottom
                    }
                }),
                hdr: match v.transfer.as_str() {
                    "smpte2084" => Some(Hdr::Pq),
                    "arib-std-b67" => Some(Hdr::Hlg),
                    _ => None,
                },
                still: v.still,
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
            // Where the content starts: after the encoder's priming.
            first_pts: a.first_secs.or(a.start_secs).unwrap_or(0.0),
            duration: (a.duration_secs.or(info.duration).unwrap_or(0.0)
                - (a.first_secs.unwrap_or(0.0) - a.start_secs.unwrap_or(0.0)))
            .max(0.0),
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
    use std::path::Path;

    use super::frame_rate;
    use vidcut_core::Env;

    #[test]
    fn only_regular_files_are_media() {
        // A pipe would block the probe until something writes to it.
        let dir = std::env::temp_dir().join(format!("vidcut-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("pipe.mp4");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        for path in [std::path::Path::new("/dev/null"), Path::new("/tmp"), &fifo] {
            let e = super::AvEnv.probe(path).unwrap_err();
            assert!(e.message.contains("not a regular file"), "{path:?}: {e}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

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
