//! The real [`Env`]: media facts from the FFmpeg libraries via vidcut-av.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use vidcut_core::analysis::{AudioLevels, Loudness, PictureStats};
use vidcut_core::editor::GeneratedProxy;
use vidcut_core::model::{
    AudioInfo, FieldOrder, Hdr, Media, MediaInfo, StartTimecode, VideoInfo, proxy_size,
};
use vidcut_core::render::RenderPlan;
use vidcut_core::time::Rational;
use vidcut_core::{Env, Error, ProbedMedia, Result};

use crate::paths;

/// The real [`Env`]. `project_dir` is the folder of the open project file,
/// when a session has one: proxies are written to `proxies/` beside it.
#[derive(Clone, Debug, Default)]
pub struct AvEnv {
    project_dir: Option<PathBuf>,
}

impl AvEnv {
    pub fn new() -> Self {
        Self::default()
    }

    /// Proxies land in `<project>/proxies`.
    pub fn for_project(project_file: &Path) -> Self {
        Self {
            project_dir: project_file.parent().map(Path::to_path_buf),
        }
    }
}

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
///
/// The sequence's `count` is not given here. image2 reads every numbered
/// file after the first; filmstrips and picture measurements stop at
/// `count` themselves.
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
        let canonical = vidcut_core::dirs::canonical(path)
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
            fingerprint: vidcut_core::fingerprint::file(&canonical).ok(),
            path: canonical,
            size: meta.len(),
            mtime_ns,
            info: convert(&info)?,
            sequence: None,
        })
    }

    fn probe_sequence(&self, path: &Path, rate: Rational) -> Result<ProbedMedia> {
        let canonical = vidcut_core::dirs::canonical(path)
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
        // Size and time stand for the whole run, once, here: a render that
        // rewrites later pictures makes a new proxy name and a stale proxy,
        // even when the first picture stays as it was.
        (probed.size, probed.mtime_ns) = (0..sequence.count)
            .filter_map(|k| std::fs::metadata(sequence.file(dir, k)).ok())
            .fold((0, 0), |(size, newest), meta| {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos() as i64);
                (size + meta.len(), newest.max(mtime))
            });
        let picture = |k: u64| sequence.file(dir, k);
        probed.fingerprint = vidcut_core::fingerprint::pictures(
            &picture(0),
            &picture(sequence.count / 2),
            &picture(sequence.count.saturating_sub(1)),
        )
        .ok();
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
        // The count is part of the cache name: a prefix of a longer numbered
        // run is not the same measurement as that run.
        let pictures = media.sequence.as_ref().map(|s| s.count);
        let key = match pictures {
            Some(n) => format!("pictures-{n}"),
            None => "pictures".to_string(),
        };
        measured(
            &PICTURES,
            paths::media_cache("analysis", media, &key),
            || vidcut_av::analysis::picture_stats(&input_of(media), pictures),
            PictureStats::to_bytes,
            PictureStats::from_bytes,
        )
    }

    fn loudness(&self, plan: &RenderPlan) -> Result<Loudness> {
        vidcut_av::analysis::loudness(plan).map_err(av_error)
    }

    fn proxy_rules(&self) -> vidcut_core::preferences::Proxies {
        // A broken preferences file fails `preferences.get` with the
        // reason; proxies go on with the defaults.
        crate::preferences::load()
            .map(|p| p.proxies)
            .unwrap_or_default()
    }

    fn proxy_dir(&self) -> PathBuf {
        match &self.project_dir {
            Some(dir) => dir.join("proxies"),
            None => paths::cache_dir().join("proxies"),
        }
    }

    fn generate_proxy(
        &self,
        media: &Media,
        longest: u32,
        dir: &Path,
        dry_run: bool,
    ) -> Result<GeneratedProxy> {
        let v = media.info.video.as_ref().ok_or_else(|| {
            Error::invalid(format!("{} has no picture to make a proxy of", media.id))
        })?;
        let (w, h) = v.display_size();
        let (w, h) = proxy_size(w, h, longest);
        // The codec is in the name: changing the preference makes a new file.
        let name = |codec: &str| {
            dir.join(format!(
                "{:016x}-{}-{}-{w}x{h}-{codec}.mkv",
                paths::fnv1a(media.path.as_os_str().as_encoded_bytes()),
                media.size,
                media.mtime_ns,
            ))
        };
        let prefs = crate::preferences::load().unwrap_or_default();
        let encoders = proxy_encoders(prefs.proxies.codec, prefs.gpu.acceleration);
        let planned = encoders.first().map_or("h264", |e| e.codec);
        let mut path = name(planned);
        let mut codec = planned;
        let mut made_with = None;
        if !dry_run && !path.is_file() {
            std::fs::create_dir_all(dir)
                .map_err(|e| Error::media(format!("{}: {e}", dir.display())))?;
            let total = Some((v.duration * v.fps.as_f64()).round().max(0.0) as u64);
            // Decoded on the GPU the viewer decodes on (preview.md §6), on a
            // stream or device of its own.
            let gpu = prefs.gpu.acceleration && prefs.gpu.decoding;
            let decoder = gpu
                .then(crate::preferences::gpu_decoder)
                .flatten()
                .and_then(|d| d.own().ok());
            let decoding = match &decoder {
                Some(d) => vidcut_av::hw::Decoding::Gpu(d),
                None if gpu => vidcut_av::hw::Decoding::Nvdec,
                None => vidcut_av::hw::Decoding::Cpu,
            };
            let written =
                vidcut_av::proxy::write(&input_of(media), w, h, &path, total, &encoders, decoding)
                    .map_err(av_error)?;
            made_with = Some(format!(
                "decoded on the {}, {}",
                written
                    .decoder
                    .map_or("CPU".into(), |k| format!("GPU ({})", k.label())),
                written.encoder.label()
            ));
            // A hardware encoder that wouldn't open gave way to another codec.
            codec = written.encoder.codec;
            if codec != planned {
                let actual = name(codec);
                std::fs::rename(&path, &actual)
                    .map_err(|e| Error::media(format!("{}: {e}", actual.display())))?;
                path = actual;
            }
        }
        Ok(GeneratedProxy {
            path,
            width: w,
            height: h,
            codec: codec.into(),
            made_with,
        })
    }

    fn open_timeline(
        &self,
        path: &Path,
        unpack: Option<&Path>,
        write: bool,
    ) -> Result<vidcut_core::editor::TimelineFile> {
        if vidcut_core::editor::is_otioz(path) {
            crate::bundle::open(path, unpack, write)
        } else {
            vidcut_core::editor::read_timeline_file(path)
        }
    }
}

/// The encoders proxies are made with here for the `codec` preference, best
/// first: software ones only without GPU acceleration (`gpu`).
pub fn proxy_encoders(
    codec: vidcut_core::preferences::ProxyCodec,
    gpu: bool,
) -> Vec<vidcut_av::proxy::ProxyEncoder> {
    let available = if gpu {
        crate::export::availability(false)
    } else {
        Default::default()
    };
    vidcut_av::proxy::encoders(codec, &available)
}

/// A measurement's failure. Cancellation uses the same flag as an export, so
/// it is `CANCELLED` and not a media error.
fn av_error(e: vidcut_av::error::Error) -> Error {
    if vidcut_av::export::is_cancelled(&e) {
        Error::new(vidcut_core::ErrorCode::Cancelled, "cancelled")
    } else {
        crate::export::av_error(&e, vidcut_core::ErrorCode::Media, String::new())
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
                mastering: v.hdr.mastering,
                light: v.hdr.light,
                hdr10plus: v.hdr.hdr10plus,
                dolby_vision: v.hdr.dolby_vision,
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
        .collect::<Vec<_>>();
    let timecode = start_timecode(
        info.timecode.as_deref(),
        info.time_reference,
        video.as_ref(),
        &audio,
    );
    Ok(MediaInfo {
        format: info.format.clone(),
        duration: info.duration.unwrap_or(0.0),
        start_time: info.start_time.unwrap_or(0.0),
        video,
        audio,
        timecode,
    })
}

/// The start timecode a file records: its timecode in the video's frames,
/// else a broadcast WAV's time reference in samples. Stills have none.
pub fn start_timecode(
    timecode: Option<&str>,
    time_reference: Option<u64>,
    video: Option<&VideoInfo>,
    audio: &[AudioInfo],
) -> Option<StartTimecode> {
    match video {
        Some(v) if v.still => None,
        Some(v) => {
            let tc = timecode?;
            let count = vidcut_core::time::parse_media_timecode(tc, v.fps).filter(|&c| c > 0)?;
            Some(StartTimecode {
                count,
                rate: v.fps,
                // Written `01:00:00;00`: counted drop-frame (decision D47).
                drop_frame: tc.contains([';', '.']) && v.fps.has_drop_frame(),
            })
        }
        None => {
            let rate = audio.first().map(|a| a.sample_rate)?;
            let count = i64::try_from(time_reference?).ok().filter(|&c| c > 0)?;
            Some(StartTimecode {
                count,
                rate: Rational::new(i64::from(rate), 1).ok()?,
                drop_frame: false,
            })
        }
    }
}

/// Fills in the start timecodes of the media `ids` that were probed before
/// vidcut kept them (cheap: the container's tags, no decoding).
pub fn fill_timecodes(p: &mut vidcut_core::Project, ids: &[String]) {
    for m in &mut p.media {
        if !ids.contains(&m.id) || m.info.timecode.is_some() || m.sequence.is_some() || m.is_still()
        {
            continue;
        }
        if let Ok((tc, reference)) = vidcut_av::probe::clock(&m.path) {
            m.info.timecode = start_timecode(
                tc.as_deref(),
                reference,
                m.info.video.as_ref(),
                &m.info.audio,
            );
        }
    }
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
