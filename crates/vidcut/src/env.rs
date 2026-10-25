use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use vidcut_core::analysis::{AudioLevels, Loudness, PictureStats, VoiceActivity};
use vidcut_core::disk_cache::{self, Kind};
use vidcut_core::editor::GeneratedProxy;
use vidcut_core::fingerprint::Origin;
use vidcut_core::lru::Lru;
use vidcut_core::model::{
    AudioInfo, FieldOrder, Hdr, Media, MediaInfo, StartTimecode, VideoInfo, proxy_size,
};
use vidcut_core::render::RenderPlan;
use vidcut_core::time::Rational;
use vidcut_core::transcript::{Asked, Speech};
use vidcut_core::transcripts::Kept;
use vidcut_core::{Env, Error, ProbedMedia, Result};

use crate::paths;

#[derive(Clone, Debug, Default)]
pub struct AvEnv {
    project_dir: Option<PathBuf>,
    heard_only: bool,
}

impl AvEnv {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn for_project(project_file: &Path) -> Self {
        Self {
            project_dir: project_file.parent().map(Path::to_path_buf),
            heard_only: false,
        }
    }

    pub fn heard_only(self) -> Self {
        Self {
            heard_only: true,
            ..self
        }
    }
}

const HEARD_KEPT: usize = 8;
static HEARD: Mutex<Lru<String, Arc<Speech>, HEARD_KEPT>> = Mutex::new(Lru::new());
static PAID: Mutex<Lru<String, Vec<Arc<Speech>>, HEARD_KEPT>> = Mutex::new(Lru::new());
const LOUD_KEPT: usize = 4096;
static LOUD: Mutex<Lru<String, Loudness, LOUD_KEPT>> = Mutex::new(Lru::new());

fn loud_key(plan: &RenderPlan) -> String {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "audio": plan.audio,
        "ducking": plan.ducking,
        "frames": plan.frames,
        "fps": plan.fps,
        "sample_rate": plan.sample_rate,
        "channels": plan.channels,
    }))
    .unwrap_or_default();
    vidcut_core::fingerprint::digest(&bytes)
}

fn heard_key(plan: &RenderPlan, asked: &Asked) -> String {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "audio": plan.audio,
        "frames": plan.frames,
        "fps": plan.fps,
        "model": asked.model,
        "model_file": asked.model_file,
        "vad_file": asked.vad_file,
        "language": asked.language,
        "find": asked.find.unwrap_or_default(),
        "sound_db": asked.level(),
        "speakers": asked.speakers,
        "speaker_model": asked.speaker_model,
        "xai": asked.xai,
        "google": asked.google,
    }))
    .unwrap_or_default();
    vidcut_core::fingerprint::digest(&bytes)
}

const LEVELS_PER_SECOND: u32 = 100;

type Memo<T> = LazyLock<Mutex<HashMap<(PathBuf, Origin), Arc<T>>>>;
static LEVELS: Memo<AudioLevels> = LazyLock::new(Default::default);
static PICTURES: Memo<PictureStats> = LazyLock::new(Default::default);
static VOICES: Memo<VoiceActivity> = LazyLock::new(Default::default);
type Transcripts = HashMap<(Origin, u32), Vec<Kept>>;
static TRANSCRIPTS: LazyLock<Mutex<Transcripts>> = LazyLock::new(Default::default);

static MADE_WITH: LazyLock<Mutex<HashMap<PathBuf, (String, vidcut_core::editor::Copied)>>> =
    LazyLock::new(Default::default);

fn measured<T>(
    memo: &Memo<T>,
    (file, origin): (PathBuf, Origin),
    measure: impl FnOnce() -> vidcut_av::error::Result<T>,
    to_bytes: impl Fn(&T) -> Vec<u8>,
    from_bytes: impl Fn(&[u8]) -> Option<T>,
) -> Result<Arc<T>> {
    let lock = || memo.lock().unwrap_or_else(PoisonError::into_inner);
    let key = (file, origin);
    if let Some(v) = lock().get(&key) {
        return Ok(v.clone());
    }
    let (file, origin) = &key;
    let read = disk_cache::read(file);
    let value = match read.as_deref().and_then(|b| from_bytes(origin.check(b)?)) {
        Some(v) => v,
        None => {
            let v = measure().map_err(av_error)?;
            let _ = disk_cache::write(file, &origin.stamp(&to_bytes(&v)));
            v
        }
    };
    let value = Arc::new(value);
    lock().insert(key, value.clone());
    Ok(value)
}

fn kept<T>(memo: &Memo<T>, key: (PathBuf, Origin)) -> Option<Arc<T>> {
    memo.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
        .cloned()
}

pub fn kept_measure(
    media: &Media,
    stream: u32,
    detect: vidcut_core::model::DuckDetect,
) -> Option<vidcut_core::duck::Measured> {
    use vidcut_core::duck::Measured;
    use vidcut_core::model::DuckDetect;
    match detect {
        DuckDetect::Speech => {
            kept(&VOICES, analysis_file(media, &format!("speech{stream}"))).map(Measured::Speech)
        }
        DuckDetect::Level => {
            kept(&LEVELS, analysis_file(media, &format!("levels{stream}"))).map(Measured::Levels)
        }
    }
}

fn transcript_file(origin: &Origin, stream: u32, key: &str) -> PathBuf {
    Kind::Analysis.file(origin, &format!("words{stream}-{key}"))
}

fn kept_transcripts(origin: &Origin, stream: u32) -> Vec<Kept> {
    let prefix = format!("{}.words{stream}-", origin.name);
    let Ok(dir) = std::fs::read_dir(Kind::Analysis.dir()) else {
        return Vec::new();
    };
    let mut kept: Vec<(SystemTime, Kept)> = dir
        .flatten()
        .filter_map(|e| {
            let key = e.file_name().to_str()?.strip_prefix(&prefix)?.to_string();
            let modified = e.metadata().ok()?.modified().ok()?;
            let bytes = disk_cache::read(&e.path())?;
            let speech = serde_json::from_slice::<Speech>(origin.check(&bytes)?).ok()?;
            let made = modified.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
            let speech = Arc::new(speech);
            Some((modified, Kept { key, made, speech }))
        })
        .collect();
    kept.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.key.cmp(&b.1.key)));
    kept.into_iter().map(|(_, k)| k).collect()
}

pub fn transcribed(media: &[Media]) -> HashSet<String> {
    let Ok(dir) = std::fs::read_dir(Kind::Analysis.dir()) else {
        return HashSet::new();
    };
    let names: HashSet<String> = dir
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let (origin, ext) = name.split_once('.')?;
            ext.strip_prefix("words")?.split_once('-')?;
            Some(origin.to_string())
        })
        .collect();
    media
        .iter()
        .filter(|m| names.contains(&*paths::media_origin(m).name))
        .map(|m| m.id.clone())
        .collect()
}

fn analysis_file(media: &Media, what: &str) -> (PathBuf, Origin) {
    let origin = paths::media_origin(media);
    (Kind::Analysis.file(&origin, what), origin)
}

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
        if vidcut_core::subtitles::is_file(&canonical) {
            let bytes = std::fs::read(&canonical)
                .map_err(|e| Error::media(format!("{}: {e}", path.display())))?;
            let (info, file) = vidcut_core::subtitles::pool_item(&bytes, &canonical)?;
            return Ok(ProbedMedia {
                fingerprint: vidcut_core::fingerprint::file(&canonical).ok(),
                path: canonical,
                size: meta.len(),
                mtime_ns,
                info,
                sequence: None,
                subtitles: Some(file),
            });
        }
        let decoder = crate::preferences::gpu_decoding().own_decoder();
        let info = vidcut_av::probe::probe(&canonical, decoder.as_ref())
            .map_err(|e| Error::media(e.to_string()))?;
        Ok(ProbedMedia {
            fingerprint: vidcut_core::fingerprint::file(&canonical).ok(),
            path: canonical,
            size: meta.len(),
            mtime_ns,
            info: convert(&info)?,
            sequence: None,
            subtitles: None,
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
            (v.fps, v.vfr) = (rate, false);
        }
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
            analysis_file(media, &format!("levels{stream}")),
            || vidcut_av::analysis::audio_levels(&media.path, stream, LEVELS_PER_SECOND),
            AudioLevels::to_bytes,
            AudioLevels::from_bytes,
        )
    }

    fn picture_stats(&self, media: &Media) -> Result<Arc<PictureStats>> {
        let pictures = media.sequence.as_ref().map(|s| s.count);
        let key = match pictures {
            Some(n) => format!("pictures-{n}"),
            None => "pictures".to_string(),
        };
        measured(
            &PICTURES,
            analysis_file(media, &key),
            || {
                let decoder = crate::preferences::gpu_decoding().own_decoder();
                vidcut_av::analysis::picture_stats(&input_of(media), pictures, decoder.as_ref())
            },
            PictureStats::to_bytes,
            PictureStats::from_bytes,
        )
    }

    fn voice_isolation(&self, download: bool) -> Result<()> {
        vidcut_av::isolate::get_ready(&paths::models_dir(), download).map_err(av_error)
    }

    fn ai_upscaler(&self, upscaler: vidcut_core::model::Upscaler, download: bool) -> Result<()> {
        vidcut_av::ai::get_ready(&paths::models_dir(), upscaler, download).map_err(av_error)
    }

    fn voice_activity(
        &self,
        media: &Media,
        stream: u32,
        download: bool,
    ) -> Result<Arc<VoiceActivity>> {
        measured(
            &VOICES,
            analysis_file(media, &format!("speech{stream}")),
            || vidcut_av::transcribe::voice_activity(media, stream, &paths::models_dir(), download),
            VoiceActivity::to_bytes,
            VoiceActivity::from_bytes,
        )
    }

    fn loudness(&self, plan: &RenderPlan) -> Result<Loudness> {
        let lock = || LOUD.lock().unwrap_or_else(PoisonError::into_inner);
        let key = loud_key(plan);
        if self.heard_only
            && let Some(l) = lock().get(&key)
        {
            return Ok(*l);
        }
        let l = vidcut_av::analysis::loudness(plan).map_err(av_error)?;
        lock().insert(key, l);
        Ok(l)
    }

    fn transcribe(&self, plan: &RenderPlan, asked: &Asked) -> Result<Arc<Speech>> {
        let lock = || HEARD.lock().unwrap_or_else(PoisonError::into_inner);
        let key = heard_key(plan, asked);
        let reuse = self.heard_only || !asked.model.paid();
        if reuse && let Some(s) = lock().get(&key) {
            return Ok(s.clone());
        }
        if self.heard_only {
            return Err(Error::conflict(
                "the timeline's sound changed while it was being transcribed; run it again",
            ));
        }
        let speech = vidcut_av::transcribe::transcribe(plan, asked, &speech_options(asked)?)
            .map_err(av_error)?;
        let speech = Arc::new(speech);
        lock().insert(key, speech.clone());
        Ok(speech)
    }

    fn media_details(&self, media: &Media) -> Result<vidcut_core::model::MediaDetails> {
        vidcut_av::probe::details(&media.path)
            .map_err(|e| Error::media(format!("{}: {e}", media.path.display())))
    }

    fn transcripts(&self, media: &Media, stream: u32) -> Result<Vec<Kept>> {
        let lock = || TRANSCRIPTS.lock().unwrap_or_else(PoisonError::into_inner);
        let key = (paths::media_origin(media), stream);
        if let Some(kept) = lock().get(&key) {
            return Ok(kept.clone());
        }
        let kept = kept_transcripts(&key.0, stream);
        lock().insert(key, kept.clone());
        Ok(kept)
    }

    fn remove_transcript(&self, media: &Media, stream: u32, key: &str) -> Result<()> {
        let origin = paths::media_origin(media);
        let file = transcript_file(&origin, stream, key);
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::media(format!("{}: {e}", file.display()))),
        }
        TRANSCRIPTS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&(origin, stream));
        Ok(())
    }

    fn transcribe_media(
        &self,
        media: &Media,
        stream: u32,
        plan: &RenderPlan,
        asked: &Asked,
    ) -> Result<Arc<Speech>> {
        let speech = self.transcribe(plan, asked)?;
        keep_transcript(media, stream, speech)
    }

    fn transcribe_files(
        &self,
        files: &[(&Media, u32, &RenderPlan)],
        asked: &Asked,
    ) -> Result<Vec<Arc<Speech>>> {
        if !asked.model.paid() || asked.model_file.is_some() {
            return files
                .iter()
                .map(|(m, s, plan)| self.transcribe_media(m, *s, plan, asked))
                .collect();
        }
        let plans: Vec<&RenderPlan> = files.iter().map(|f| f.2).collect();
        let lock = || PAID.lock().unwrap_or_else(PoisonError::into_inner);
        let key = plans
            .iter()
            .map(|p| heard_key(p, asked))
            .collect::<Vec<_>>()
            .join(" ");
        if self.heard_only {
            return lock().get(&key).cloned().ok_or_else(|| {
                Error::conflict("the files changed while they were being transcribed; run it again")
            });
        }
        let speeches =
            vidcut_av::transcribe::transcribe_files(&plans, asked, &speech_options(asked)?)
                .map_err(av_error)?;
        let kept = files
            .iter()
            .zip(speeches)
            .map(|((m, s, _), speech)| keep_transcript(m, *s, Arc::new(speech)))
            .collect::<Result<Vec<_>>>()?;
        lock().insert(key, kept.clone());
        Ok(kept)
    }

    fn fillers(&self, language: &str) -> Option<Vec<String>> {
        crate::preferences::fillers(language)
    }

    fn looks(&self, kind: vidcut_core::looks::LookKind) -> vidcut_core::looks::Listing {
        vidcut_core::looks::read(&vidcut_core::looks::dir(), kind)
    }

    fn proxy_rules(&self) -> vidcut_core::preferences::Proxies {
        crate::preferences::load()
            .map(|p| p.proxies)
            .unwrap_or_default()
    }

    fn proxy_dir(&self) -> PathBuf {
        let dir = self.project_dir.clone().unwrap_or_else(paths::cache_dir);
        self.proxy_rules().folder_in(&dir)
    }

    fn generate_proxy(
        &self,
        media: &Media,
        longest: u32,
        codec: Option<&str>,
        dir: &Path,
        dry_run: bool,
    ) -> Result<GeneratedProxy> {
        let v = media.info.video.as_ref().ok_or_else(|| {
            Error::invalid(format!("{} has no picture to make a proxy of", media.id))
        })?;
        let (w, h) = v.display_size();
        let (w, h) = proxy_size(w, h, longest);
        let origin = paths::proxy_origin(media);
        let name = |codec: &str| dir.join(format!("{}-{w}x{h}-{codec}.mkv", origin.name));
        let prefs = crate::preferences::load().unwrap_or_default();
        let (gpu, decoding) = (
            prefs.gpu.acceleration,
            prefs.gpu.acceleration && prefs.gpu.decoding,
        );
        let encoders = codec
            .and_then(proxy_codec)
            .map(|c| proxy_encoders(c, gpu, decoding))
            .filter(|e| !e.is_empty())
            .unwrap_or_else(|| proxy_encoders(prefs.proxies.codec, gpu, decoding));
        let planned = encoders.first().map_or("h264", |e| e.codec);
        let mut path = name(planned);
        let mut codec = planned;
        let made = || MADE_WITH.lock().unwrap_or_else(PoisonError::into_inner);
        let mut made_with = None;
        let mut copies = None;
        if !dry_run
            && path.is_file()
            && vidcut_av::proxy::origin(&path).as_deref() == Some(&*origin.text)
        {
            (made_with, copies) = made().remove(&path).unzip();
            disk_cache::used(&path);
        } else if !dry_run {
            if disk_cache::room(&path) == Some(0) {
                return Err(no_room(dir));
            }
            std::fs::create_dir_all(dir)
                .map_err(|e| Error::media(format!("{}: {e}", dir.display())))?;
            let total = Some((v.duration * v.fps.as_f64()).round().max(0.0) as u64);
            let decoder = crate::preferences::gpu_decoding().own_decoder();
            let written = vidcut_av::proxy::write(
                &input_of(media),
                w,
                h,
                &path,
                total,
                &encoders,
                decoder.as_ref(),
                &origin.text,
                media.color_space,
            )
            .map_err(av_error)?;
            let went = written
                .went
                .and_then(vidcut_av::route::Went::label)
                .map_or(String::new(), |w| format!(", {w}"));
            let how = format!(
                "decoded on the {}{went}, {}",
                written
                    .decoder
                    .map_or("CPU".into(), |k| format!("GPU ({})", k.label())),
                written.encoder.label()
            );
            codec = written.encoder.codec;
            if codec != planned {
                let actual = name(codec);
                std::fs::rename(&path, &actual)
                    .map_err(|e| Error::media(format!("{}: {e}", actual.display())))?;
                path = actual;
            }
            if !disk_cache::added(&path) {
                return Err(no_room(dir));
            }
            made().insert(path.clone(), (how.clone(), written.copies));
            made_with = Some(how);
            copies = Some(written.copies);
        }
        Ok(GeneratedProxy {
            path,
            width: w,
            height: h,
            codec: codec.into(),
            made_with,
            copies,
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

fn proxy_codec(codec: &str) -> Option<vidcut_core::preferences::ProxyCodec> {
    serde_json::from_value(serde_json::Value::String(codec.into()))
        .ok()
        .filter(|c| *c != vidcut_core::preferences::ProxyCodec::Auto)
}

pub fn proxy_encoders(
    codec: vidcut_core::preferences::ProxyCodec,
    gpu: bool,
    decoding: bool,
) -> Vec<vidcut_av::proxy::ProxyEncoder> {
    let available = if gpu {
        crate::export::availability(false)
    } else {
        Default::default()
    };
    let plays = match codec {
        vidcut_core::preferences::ProxyCodec::Auto => crate::export::plays(decoding),
        _ => None,
    };
    let plays: Vec<(&str, vidcut_av::hw::Kind)> = plays
        .iter()
        .flatten()
        .filter_map(|(codec, decoder)| {
            Some((codec.as_str(), vidcut_av::hw::Kind::with_label(decoder)?))
        })
        .collect();
    vidcut_av::proxy::encoders(codec, &available, &plays)
}

fn av_error(e: vidcut_av::error::Error) -> Error {
    if vidcut_av::export::is_cancelled(&e) {
        Error::new(vidcut_core::ErrorCode::Cancelled, "cancelled")
    } else {
        crate::export::av_error(&e, vidcut_core::ErrorCode::Media, String::new())
    }
}

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
            let drop_frame = tc.contains([';', '.']) && v.fps.has_drop_frame();
            let count = vidcut_core::time::parse_media_timecode(tc, v.fps)
                .filter(|&c| c > 0 || drop_frame)?;
            Some(StartTimecode {
                count,
                rate: v.fps,
                drop_frame,
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

fn speech_options(asked: &Asked) -> Result<vidcut_av::transcribe::Options<'static>> {
    static MODELS: LazyLock<PathBuf> = LazyLock::new(paths::models_dir);
    let key = match asked.model.paid() && asked.model_file.is_none() {
        true => crate::services::key(asked.model)?,
        false => None,
    };
    Ok(vidcut_av::transcribe::Options {
        models: &MODELS,
        gpu: crate::preferences::gpu_acceleration(),
        key,
    })
}

fn no_room(dir: &Path) -> Error {
    Error::media(format!(
        "{}: the disk cache has no room for this proxy: it stays under cache.limit and keeps \
         {}% of the disk free; free some space, or set proxies.folder outside the cache dir",
        dir.display(),
        disk_cache::KEEP_FREE_PERCENT
    ))
}

fn keep_transcript(media: &Media, stream: u32, speech: Arc<Speech>) -> Result<Arc<Speech>> {
    vidcut_core::transcript::spoken(&speech).map_err(Error::invalid)?;
    let origin = paths::media_origin(media);
    let file = transcript_file(&origin, stream, &vidcut_core::transcripts::key(&speech));
    let bytes = serde_json::to_vec(&*speech).map_err(|e| Error::internal(e.to_string()))?;
    disk_cache::write(&file, &origin.stamp(&bytes))
        .map_err(|e| Error::media(format!("{}: {e}", file.display())))?;
    TRANSCRIPTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&(origin, stream));
    Ok(speech)
}

#[cfg(test)]
mod tests {
    use super::{frame_rate, proxy_codec, start_timecode};
    use vidcut_core::model::{StartTimecode, VideoInfo};
    use vidcut_core::preferences::ProxyCodec;
    use vidcut_core::time::Rational;

    #[test]
    fn a_proxy_names_its_codec_as_the_preference_does() {
        assert_eq!(proxy_codec("dnxhr"), Some(ProxyCodec::Dnxhr));
    }

    #[test]
    fn a_drop_frame_start_at_zero_says_how_it_counts() {
        let fps = Rational::new(30000, 1001).unwrap();
        let video = VideoInfo {
            codec: "h264".into(),
            width: 1920,
            height: 1080,
            sar: [1, 1],
            pix_fmt: "yuv420p".into(),
            fps,
            vfr: false,
            first_pts: 0.0,
            duration: 10.0,
            rotation: 0,
            fields: None,
            hdr: None,
            still: false,
            mastering: None,
            light: None,
            hdr10plus: false,
            dolby_vision: None,
        };
        let tc = start_timecode(Some("00:00:00;00"), None, Some(&video), &[]);
        let want = StartTimecode {
            count: 0,
            rate: fps,
            drop_frame: true,
        };
        assert_eq!(tc, Some(want));
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
