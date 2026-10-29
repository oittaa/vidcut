use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use transcribe_cpp as tc;
use vidcut_core::analysis::VoiceActivity;
use vidcut_core::model::Media;
use vidcut_core::render::{AudioPiece, RenderPlan};
use vidcut_core::request::{FindSpeech, SpeakerModel, SpeechModel};
use vidcut_core::transcript::{self, Asked, RATE, Said, Speech, Timing, Turn, Word};

use crate::error::{Error, Result};
use crate::export::audio::AudioMix;
use crate::export::{CANCELLED, DECLICK_MS, Note, Progress, SpeechNote, Stage};
use crate::models::{Fetch, File, Need, fetch, listed, network, present, size};

use vidcut_core::catalog::{self, Measure};
pub use vidcut_core::catalog::{
    MOSS_GGUF as MOSS, PARAKEET_ULTRA_GGUF as PARAKEET_ULTRA, QWEN3_ASR_GGUF as QWEN3_ASR, RUNTIME,
    Runtime, SILERO_VAD_GGUF as SILERO_VAD, SORTFORMER_GGUF as SORTFORMER,
    WHISPER_LARGE_V3_GGUF as WHISPER_LARGE_V3,
};

pub fn speaker_file(language: Option<&str>, choice: SpeakerModel) -> &'static File {
    match choice {
        SpeakerModel::Moss => &MOSS,
        SpeakerModel::Sortformer => &SORTFORMER,
        SpeakerModel::Auto if language.is_some_and(|l| catalog::MOSS.languages.contains(&l)) => {
            &MOSS
        }
        SpeakerModel::Auto => &SORTFORMER,
    }
}

pub fn speakers_by(language: Option<&str>, choice: SpeakerModel) -> Option<&'static File> {
    let unheard = language.is_none() && choice == SpeakerModel::Auto;
    (!unheard).then(|| speaker_file(language, choice))
}

pub fn speaker_name(f: &File) -> &'static str {
    match f == &MOSS {
        true => SpeakerModel::Moss.name(),
        false => SpeakerModel::Sortformer.name(),
    }
}

pub(crate) struct Telling(SpeechNote);

impl Telling {
    pub(crate) fn new(asked: &Asked, speakers_by: &str, device: Option<String>) -> Self {
        let told = SpeechNote {
            model: asked.model_name(),
            find: asked.find.unwrap_or_default(),
            device,
            language: asked.language.clone(),
            speakers_by: asked.speakers.then(|| speakers_by.to_string()),
        };
        crate::progress::note(Note::Speech(told.clone()));
        Self(told)
    }

    fn here(asked: &Asked) -> Self {
        let by = speakers_by(asked.language.as_deref(), asked.speaker_model)
            .map_or(SpeakerModel::Auto.name(), speaker_name);
        Self::new(asked, by, None)
    }

    pub(crate) fn tell(&mut self, change: impl FnOnce(&mut SpeechNote)) {
        let before = self.0.clone();
        change(&mut self.0);
        if self.0 != before {
            crate::progress::note(Note::Speech(self.0.clone()));
        }
    }

    pub(crate) fn heard(&mut self, language: &str, choice: SpeakerModel) {
        self.tell(|n| {
            if n.language.is_none() {
                n.language = Some(language.to_string());
            }
            if n.speakers_by.as_deref() == Some(SpeakerModel::Auto.name()) {
                n.speakers_by = Some(speaker_name(speaker_file(Some(language), choice)).into());
            }
        });
    }
}

fn needed_for(dir: &Path, files: &[&File], vad: bool) -> Vec<Need> {
    let mut out = Vec::new();
    if let Some(r) = RUNTIME.filter(|r| !r.present(dir)) {
        out.push(Need {
            title: "transcribe.cpp's library".into(),
            bytes: Some(r.bytes),
            system: false,
        });
    }
    let wanted = vad
        .then_some(&SILERO_VAD)
        .into_iter()
        .chain(files.iter().copied());
    for f in wanted.filter(|f| !present(dir, f)) {
        out.push(Need {
            title: f.title.into(),
            bytes: Some(f.bytes),
            system: false,
        });
    }
    out
}

pub fn needed(dir: &Path, files: &[&File]) -> Vec<Need> {
    needed_for(dir, files, true)
}

pub fn wanted(asked: &Asked) -> Vec<&'static File> {
    let model = asked
        .model_file
        .is_none()
        .then(|| file_of(asked.model))
        .flatten();
    model
        .into_iter()
        .chain(
            (asked.speakers && !speakers_from_service(asked))
                .then(|| speaker_file(asked.language.as_deref(), asked.speaker_model)),
        )
        .collect()
}

fn speakers_from_service(asked: &Asked) -> bool {
    asked.model_file.is_none() && asked.model.paid()
}

pub fn file_of(model: SpeechModel) -> Option<&'static File> {
    match model {
        SpeechModel::Qwen3Asr => Some(&QWEN3_ASR),
        SpeechModel::WhisperLargeV3 => Some(&WHISPER_LARGE_V3),
        SpeechModel::ParakeetUltra => Some(&PARAKEET_ULTRA),
        SpeechModel::Xai | SpeechModel::Google => None,
    }
}

pub struct Options<'a> {
    pub models: &'a Path,
    pub gpu: bool,
    pub key: Option<String>,
}

pub fn run_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    format!("r{:x}{:x}", nanos, NEXT.fetch_add(1, Ordering::Relaxed))
}

const PAD: f64 = 0.2;
const TELLING_TICK: Duration = Duration::from_millis(250);
const UNTIMED: f64 = catalog::PIECES.untimed_secs;
const SHORTEST: f64 = catalog::PIECES.shortest_secs;
const LONGEST: f64 = catalog::PIECES.longest_secs;
const QWEN3_BASE_MB: f64 = catalog::PIECES.qwen3_base_mb;
const QWEN3_MB_PER_SECOND: f64 = catalog::PIECES.qwen3_mb_per_second;

pub fn longest_piece(
    arch: &str,
    timed: bool,
    gpu: bool,
    memory_mb: Option<u64>,
    max_audio_ms: i64,
) -> f64 {
    let fits = match (arch, gpu, memory_mb) {
        (_, false, _) => SHORTEST,
        ("parakeet", true, _) => LONGEST,
        ("qwen3_asr", true, Some(mb)) => (mb as f64 / 2.0 - QWEN3_BASE_MB) / QWEN3_MB_PER_SECOND,
        _ => SHORTEST,
    }
    .clamp(SHORTEST, LONGEST);
    let secs = if timed { fits } else { fits.min(UNTIMED) };
    if max_audio_ms > 0 {
        secs.min(max_audio_ms as f64 / 1000.0)
    } else {
        secs
    }
}

fn start(dir: &Path, runtime: &Runtime) -> Result<()> {
    static STARTED: Mutex<bool> = Mutex::new(false);
    let mut started = STARTED.lock().unwrap_or_else(PoisonError::into_inner);
    if *started {
        return Ok(());
    }
    let home = runtime.home(dir);
    #[cfg(windows)]
    {
        let longest = std::fs::read_dir(&home)?
            .filter_map(|e| e.ok())
            .map(|e| e.path().as_os_str().len())
            .max()
            .unwrap_or(0);
        if longest >= 260 {
            return Err(Error::Invalid(format!(
                "transcribe.cpp's library is in {}, where its files' paths pass the 260 \
                 characters Windows loads from: set XDG_CACHE_HOME to a shorter folder",
                home.display()
            )));
        }
    }
    transcribe_cpp_sys::load(&home.join(runtime.library))
        .map_err(|e| Error::Invalid(format!("transcribe.cpp's library: {e}")))?;
    if !crate::debug() {
        tc::disable_logging();
    }
    let allowed = match vulkan_loader() {
        true => tc::BackendMask::ALL,
        false => tc::BackendMask::CPU,
    };
    tc::init_backends_with(None::<&Path>, allowed).map_err(tc_error)?;
    *started = true;
    Ok(())
}

fn get_ready(dir: &Path, files: &[&File], vad: bool, download: bool) -> Result<()> {
    network();
    let Some(runtime) = RUNTIME else {
        return Err(Error::Missing(
            "transcribe.cpp publishes no library for this system, so speech can't be heard here"
                .into(),
        ));
    };
    let needs = needed_for(dir, files, vad);
    if !needs.is_empty() && !download {
        let own = match files
            .iter()
            .any(|f| !present(dir, f) && gpu_gb(f).is_some())
        {
            true => ", or name a model file of your own with model_file",
            false => "",
        };
        let total: u64 = needs.iter().filter_map(|n| n.bytes).sum();
        return Err(Error::Missing(format!(
            "{} {} on this computer yet: pass download (--download) to download {} ({}) into {}, \
             each checked against its SHA-256{own}",
            listed(&needs),
            if needs.len() == 1 { "isn't" } else { "aren't" },
            if needs.len() == 1 { "it" } else { "them" },
            size(total),
            dir.display()
        )));
    }
    if !runtime.present(dir) {
        let archive = fetch(
            &Fetch {
                url: runtime.url(),
                name: runtime.archive(),
                title: "transcribe.cpp's library".into(),
                bytes: runtime.bytes,
                sha256: runtime.sha256,
            },
            dir,
        )?;
        unpack(&archive, &runtime, dir)?;
    }
    for f in vad
        .then_some(&SILERO_VAD)
        .into_iter()
        .chain(files.iter().copied())
    {
        if !present(dir, f) {
            fetch(&Fetch::of(f), dir)?;
        }
    }
    start(dir, &runtime)
}

pub fn inspect(dir: &Path, file: &File, download: bool) -> Result<catalog::Found> {
    get_ready(dir, &[file], false, download)?;
    crate::models::verify(dir, &[*file])?;
    let path = dir.join(file.name);
    let model = match file == &SILERO_VAD {
        true => vad_model(&path)?,
        false => load(&path, false, &mut Vec::new())?.model,
    };
    let roles = model.roles();
    let mut found = catalog::Found::default();
    if roles.contains(tc::Role::Asr) {
        let caps = model.capabilities().map_err(tc_error)?;
        found.languages = caps.languages.clone();
        found.times = Some(timing_of(caps.max_timestamp_kind));
        found.audio_secs = (caps.max_audio_ms > 0).then(|| caps.max_audio_ms as f64 / 1000.0);
    }
    if roles.contains(tc::Role::Diarize) {
        let info = model.diarize_info().map_err(tc_error)?;
        found.speakers = Some(f64::from(info.max_speakers));
    }
    Ok(found)
}

fn unpack(archive: &Path, runtime: &Runtime, dir: &Path) -> Result<()> {
    let home = runtime.home(dir);
    let _held = vidcut_core::temp::lock(&home)?;
    if runtime.present(dir) {
        return Ok(());
    }
    let part = vidcut_core::temp::dir(&home)?;
    let gz = flate2::read::GzDecoder::new(std::fs::File::open(archive)?);
    tar::Archive::new(gz).unpack(part.path())?;
    let inner = std::iter::once(part.path().to_path_buf())
        .chain(
            std::fs::read_dir(part.path())?
                .filter_map(|e| e.ok())
                .map(|e| e.path()),
        )
        .find(|d| d.join(runtime.library).is_file())
        .ok_or_else(|| {
            Error::Invalid(format!(
                "{}: no {} in it",
                archive.display(),
                runtime.library
            ))
        })?;
    let _ = std::fs::remove_dir_all(&home);
    if inner == part.path() {
        part.keep(&home)?;
    } else {
        std::fs::rename(&inner, &home)?;
    }
    let _ = std::fs::remove_file(archive);
    Ok(())
}

#[cfg(windows)]
fn vulkan_loader() -> bool {
    // SAFETY: loads the Vulkan loader, which runs no code of ours; it stays loaded.
    unsafe { libloading::Library::new("vulkan-1.dll") }
        .map(std::mem::forget)
        .is_ok()
}

#[cfg(not(windows))]
fn vulkan_loader() -> bool {
    true
}

fn tc_error(e: tc::Error) -> Error {
    match e {
        tc::Error::Aborted { .. } => Error::Invalid(CANCELLED.into()),
        tc::Error::ModelFileNotFound(m) => Error::Missing(m),
        e => Error::Invalid(format!("transcribe.cpp: {e}")),
    }
}

fn report(path: &Path, stage: Stage, done: i64, total: i64, started: Instant) {
    crate::progress::report(path, Progress::stage(stage, done, total, started.elapsed()));
}

pub(crate) fn vad_path(asked: &Asked, dir: &Path) -> Result<PathBuf> {
    get_ready(
        dir,
        &wanted(asked),
        asked.vad_file.is_none(),
        asked.download,
    )?;
    let vad = asked
        .vad_file
        .clone()
        .unwrap_or_else(|| dir.join(SILERO_VAD.name));
    if !vad.is_file() {
        return Err(Error::Missing(format!("{}: no such file", vad.display())));
    }
    Ok(vad)
}

fn files(asked: &Asked, dir: &Path) -> Result<(PathBuf, PathBuf)> {
    let vad = vad_path(asked, dir)?;
    let model = match (&asked.model_file, file_of(asked.model)) {
        (Some(f), _) => f.clone(),
        (None, Some(f)) => dir.join(f.name),
        (None, None) => {
            return Err(Error::Invalid(format!(
                "{} runs on its company's servers",
                asked.model.title()
            )));
        }
    };
    if !model.is_file() {
        return Err(Error::Missing(format!("{}: no such file", model.display())));
    }
    Ok((model, vad))
}

fn listen(
    lanes: &[Vec<AudioPiece>],
    total: i64,
    path: &Path,
    started: Instant,
    mut heard: impl FnMut(&[f32]) -> Result<()>,
) -> Result<()> {
    let mut mix = AudioMix::new(lanes, RATE, 1, DECLICK_MS);
    let mut done = 0;
    while done < total {
        crate::cancel::stop()?;
        let n = (total - done).min(i64::from(RATE) * 10);
        heard(&mix.read(n as usize)?)?;
        done += n;
        let ms = |s: i64| s * 1000 / i64::from(RATE);
        report(path, Stage::Listening, ms(done), ms(total), started);
    }
    Ok(())
}

pub(crate) fn heard_plan(plan: &RenderPlan, path: &Path, started: Instant) -> Result<Vec<f32>> {
    if plan.sample_rate != RATE {
        return Err(Error::Invalid(format!(
            "speech is heard at {RATE} Hz, not {} Hz",
            plan.sample_rate
        )));
    }
    let total = plan.samples().max(0);
    let channels = usize::from(plan.channels.max(1));
    let mut pcm = Vec::with_capacity(total as usize * channels);
    let mut mix = AudioMix::new(&plan.audio, RATE, plan.channels.max(1), DECLICK_MS);
    let mut done = 0;
    while done < total {
        crate::cancel::stop()?;
        let n = (total - done).min(i64::from(RATE) * 10);
        pcm.extend_from_slice(&mix.read(n as usize)?);
        done += n;
        let ms = |s: i64| s * 1000 / i64::from(RATE);
        report(path, Stage::Listening, ms(done), ms(total), started);
    }
    Ok(pcm)
}

pub(crate) struct Found {
    pub stretches: Vec<(i64, i64)>,
    pub quiet: Vec<f32>,
    pub frame: usize,
    pub found: FindSpeech,
    pub floor: f64,
}

pub(crate) fn find_speech(
    vad: &Path,
    pcm: &[f32],
    asked: &Asked,
    warnings: &mut Vec<String>,
) -> Result<Found> {
    let (voice, frame) = speech(vad, pcm)?;
    let quiet: Vec<f32> = pcm
        .chunks(frame)
        .map(|c| (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    let found = asked.find.unwrap_or_default();
    let floor = asked.sound_db.unwrap_or(transcript::SOUND_DB);
    let sound = transcript::sounding(&quiet, frame, floor);
    let stretches = match found {
        FindSpeech::Voice => {
            if let Some(hint) = missed_singing(&voice, &sound, pcm.len()) {
                warnings.push(hint);
            }
            voice
        }
        FindSpeech::Sound => sound,
        FindSpeech::None => vec![(0, pcm.len() as i64)],
    };
    Ok(Found {
        stretches,
        quiet,
        frame,
        found,
        floor,
    })
}

pub(crate) struct Hearing<'a> {
    pub(crate) dir: &'a Path,
    pub(crate) gpu: bool,
    pub(crate) download: bool,
    pub(crate) started: Instant,
}

pub(crate) fn speakers_of(
    at: &Hearing,
    pcm: &[f32],
    language: Option<&str>,
    choice: SpeakerModel,
    told: &mut Telling,
    warnings: &mut Vec<String>,
) -> Result<(Vec<Turn>, &'static str)> {
    let (dir, gpu, started) = (at.dir, at.gpu, at.started);
    let picked = speaker_file(language, choice);
    told.tell(|n| n.speakers_by = Some(speaker_name(picked).into()));
    get_ready(dir, &[picked], false, at.download).map_err(|e| match e {
        Error::Missing(m) if picked == &MOSS => Error::Missing(format!(
            "{m}; or pick Sortformer under Speakers By (speaker_model: sortformer)"
        )),
        e => e,
    })?;
    if picked == &MOSS {
        return moss_turns(dir, pcm, gpu, started, warnings)
            .map(|turns| (turns, SpeakerModel::Moss.name()))
            .map_err(|e| match e {
                Error::Invalid(m) if m == CANCELLED => Error::Invalid(m),
                e => Error::Invalid(format!(
                    "{} couldn't tell the speakers apart: {e}. Sortformer is a smaller model to \
                     try: pick it under Speakers By (speaker_model: sortformer)",
                    MOSS.title
                )),
            });
    }
    let path = dir.join(SORTFORMER.name);
    let total = pcm.len() as i64 * 1000 / i64::from(RATE);
    report(&path, Stage::Speakers, 0, total, started);
    let loaded = load(&path, gpu, warnings)?;
    crate::cancel::stop()?;
    let most = catalog::SORTFORMER
        .value(Measure::Speakers)
        .map_or(0, |n| n as i32);
    let mut session = loaded.model.diarize_session().map_err(tc_error)?;
    let turns: Vec<Turn> = telling(&path, total, started, |token| {
        session.set_cancel_token(token);
        session.run(pcm, &tc::DiarizeOptions::default())
    })?
    .iter()
    .filter(|t| t.speaker_id > 0 && t.t1_ms > t.t0_ms)
    .map(|t| Turn {
        start: t.t0_ms as f64 / 1000.0,
        end: t.t1_ms as f64 / 1000.0,
        speaker: t.speaker_id as u32,
    })
    .collect();
    report(&path, Stage::Speakers, total, total, started);
    let could = choice == SpeakerModel::Auto && speaker_file(language, SpeakerModel::Auto) == &MOSS;
    if let Some(w) = full(&turns, most, could) {
        warnings.push(w);
    }
    Ok((turns, SpeakerModel::Sortformer.name()))
}

fn moss_turns(
    dir: &Path,
    pcm: &[f32],
    gpu: bool,
    started: Instant,
    warnings: &mut Vec<String>,
) -> Result<Vec<Turn>> {
    let path = dir.join(MOSS.name);
    let total = pcm.len() as i64 * 1000 / i64::from(RATE);
    report(&path, Stage::Speakers, 0, total, started);
    let loaded = load(&path, gpu, warnings)?;
    crate::cancel::stop()?;
    let mut session = loaded.model.session().map_err(tc_error)?;
    let heard = telling(&path, total, started, |token| {
        session.set_cancel_token(token);
        session.run(
            pcm,
            &tc::RunOptions {
                diarize: tc::Diarize::On,
                ..Default::default()
            },
        )
    })?;
    report(&path, Stage::Speakers, total, total, started);
    Ok(heard
        .speaker_segments
        .iter()
        .filter(|t| t.speaker_id > 0 && t.t1_ms > t.t0_ms)
        .map(|t| Turn {
            start: t.t0_ms as f64 / 1000.0,
            end: t.t1_ms as f64 / 1000.0,
            speaker: t.speaker_id as u32,
        })
        .collect())
}

fn telling<T: Send>(
    path: &Path,
    total: i64,
    started: Instant,
    run: impl FnOnce(&tc::CancelToken) -> std::result::Result<T, tc::Error> + Send,
) -> Result<T> {
    let token = tc::CancelToken::new();
    let waiting = std::thread::current();
    std::thread::scope(|s| {
        let work = std::thread::Builder::new()
            .name("vidcut-speakers".into())
            .spawn_scoped(s, || {
                let told = run(&token);
                waiting.unpark();
                told
            })?;
        while !work.is_finished() {
            if crate::cancel::cancelled() {
                token.cancel();
            }
            report(path, Stage::Speakers, 0, total, started);
            std::thread::park_timeout(TELLING_TICK);
        }
        work.join()
            .map_err(|_| Error::Invalid("the speaker model's thread panicked".into()))?
            .map_err(tc_error)
    })
}

fn full(turns: &[Turn], most: i32, moss_could: bool) -> Option<String> {
    let mut heard: Vec<u32> = turns.iter().map(|t| t.speaker).collect();
    heard.sort_unstable();
    heard.dedup();
    let moss = match moss_could {
        true => format!(
            ". {} ({}) tells more apart in English and Chinese: transcribe with the language \
             set to download it",
            MOSS.title,
            size(MOSS.bytes)
        ),
        false => String::new(),
    };
    (most > 0 && heard.len() >= most as usize).then(|| {
        format!(
            "{most} speakers, the most Sortformer tells apart: more people share their numbers; \
             Same Person As, Separate and Assign To fix them{moss}"
        )
    })
}

fn vad_model(vad: &Path) -> Result<tc::Model> {
    tc::Model::load_with(
        vad,
        &tc::ModelOptions {
            backend: tc::Backend::Cpu,
            device: None,
            no_mmap: false,
        },
    )
    .map_err(tc_error)
}

pub fn voice_activity(
    media: &Media,
    stream: u32,
    models: &Path,
    download: bool,
) -> Result<VoiceActivity> {
    get_ready(models, &[], true, download)?;
    let started = Instant::now();
    let a = media
        .info
        .audio
        .iter()
        .find(|a| a.index == stream)
        .ok_or_else(|| Error::Invalid(format!("{}: no audio stream {stream}", media.id)))?;
    let secs = if a.duration > 0.0 {
        a.duration
    } else {
        media.info.duration
    };
    let total = (secs * f64::from(RATE)).ceil() as i64;
    let lane = vec![AudioPiece::Source {
        media: media.id.clone(),
        path: media.path.clone(),
        stream,
        origin: a.first_pts,
        source_sample: 0,
        samples: total,
        gain_db: 0.0,
        speed: 1.0,
        joined: false,
        fade: None,
        isolation: None,
        keyed: None,
    }];
    let model = vad_model(&models.join(SILERO_VAD.name))?;
    let frame = model.vad_info().map_err(tc_error)?.frame_samples.max(1);
    let mut session = model.vad_session().map_err(tc_error)?;
    let mut speech = Vec::new();
    listen(&[lane], total, &media.path, started, |pcm| {
        speech.extend(session.stream_feed(pcm).map_err(tc_error)?.probs);
        Ok(())
    })?;
    speech.extend(session.stream_flush().map_err(tc_error)?.probs);
    Ok(VoiceActivity {
        rate: RATE,
        frame: frame as u32,
        start: a.first_pts,
        speech,
    })
}

fn missed_singing(voice: &[(i64, i64)], sound: &[(i64, i64)], total: usize) -> Option<String> {
    let length = |spans: &[(i64, i64)]| spans.iter().map(|(a, b)| b - a).sum::<i64>();
    let (heard, loud) = (length(voice), length(sound));
    let mostly_loud = loud * 2 >= total as i64 && loud > 0;
    (mostly_loud && heard * 4 < loud).then(|| {
        format!(
            "the voice detection found speech in {}% of the sound ({} of {}): singing or speech \
             over music may have been left out; find the speech by sound level instead \
             (find: sound)",
            heard * 100 / loud,
            transcript_clock(secs(heard)),
            transcript_clock(secs(loud))
        )
    })
}

fn speech(vad: &Path, pcm: &[f32]) -> Result<(Vec<(i64, i64)>, usize)> {
    let model = vad_model(vad)?;
    let frame = model.vad_info().map_err(tc_error)?.frame_samples.max(1) as usize;
    let found = model
        .vad_session()
        .and_then(|mut s| s.run(pcm, &tc::VadOptions::default()))
        .map_err(tc_error)?;
    Ok((
        found
            .segments
            .iter()
            .map(|s| (s.start_sample, s.end_sample))
            .collect(),
        frame,
    ))
}

struct Loaded {
    model: tc::Model,
    device: String,
    memory_mb: Option<u64>,
    gpu: bool,
}

fn gpu_device() -> Option<(tc::Device, Option<u64>)> {
    let devices: Vec<tc::Device> = tc::devices()
        .into_iter()
        .filter(|d| matches!(d.device_type, tc::DeviceType::Gpu | tc::DeviceType::Igpu))
        .collect();
    if let Some(g) = crate::gpus::in_use() {
        let by_slot = devices.iter().find(|d| {
            g.slot.is_some() && d.device_id.as_deref().and_then(crate::gpus::parse_slot) == g.slot
        });
        let by_name = || {
            devices
                .iter()
                .find(|d| d.description.trim() == g.name.trim())
        };
        if let Some(d) = by_slot.or_else(by_name) {
            return Some((d.clone(), g.memory_mb));
        }
    }
    devices
        .iter()
        .find(|d| d.device_type == tc::DeviceType::Gpu)
        .or(devices.first())
        .map(|d| (d.clone(), None))
}

fn device_label(device: &tc::Device) -> String {
    format!("{} ({})", device.description, kind_name(&device.kind))
}

pub fn gpu_label() -> Option<String> {
    RUNTIME?;
    let gpu = crate::gpus::in_use()?;
    #[cfg(target_os = "macos")]
    let kind = "metal";
    #[cfg(not(target_os = "macos"))]
    let kind = "vulkan";
    Some(format!("{} ({})", gpu.name, kind_name(kind)))
}

fn kind_name(kind: &str) -> String {
    match kind {
        "vulkan" => "Vulkan".into(),
        "metal" => "Metal".into(),
        k => k.to_string(),
    }
}

fn load(path: &Path, gpu: bool, warnings: &mut Vec<String>) -> Result<Loaded> {
    if gpu {
        match gpu_device() {
            Some((device, memory_mb)) => {
                let backend = match device.kind.as_str() {
                    "metal" => tc::Backend::Metal,
                    _ => tc::Backend::Vulkan,
                };
                let label = device_label(&device);
                match tc::Model::load_with(
                    path,
                    &tc::ModelOptions {
                        backend,
                        device: Some(device),
                        no_mmap: false,
                    },
                ) {
                    Ok(model) => {
                        return Ok(Loaded {
                            model,
                            device: label,
                            memory_mb,
                            gpu: true,
                        });
                    }
                    Err(e) => warnings.push(format!("{label}: {e}; transcribed on the CPU")),
                }
            }
            None => warnings.push("no GPU transcribe.cpp can use: transcribed on the CPU".into()),
        }
    }
    let model = tc::Model::load_with(
        path,
        &tc::ModelOptions {
            backend: tc::Backend::Cpu,
            device: None,
            no_mmap: false,
        },
    )
    .map_err(tc_error)?;
    Ok(Loaded {
        model,
        device: "CPU".into(),
        memory_mb: None,
        gpu: false,
    })
}

fn gigabytes(mb: f64) -> String {
    let gb = format!("{:.1}", (mb / 1024.0 * 10.0).floor() / 10.0);
    gb.trim_end_matches(".0").to_string()
}

fn gpu_gb(f: &File) -> Option<f64> {
    catalog::holding(f)?.value(Measure::GpuGb)
}

pub fn short_of_memory(f: &File) -> Option<String> {
    let model = catalog::holding(f)?;
    let have = crate::gpus::in_use()?.memory_mb?;
    let verdict = catalog::check(model, &[(Measure::GpuGb, have as f64 / 1024.0)]);
    let short = verdict.warnings.first()?;
    Some(format!(
        "{} needs about {} GB of GPU memory; this GPU has {} GB",
        f.title,
        short.limit.value,
        gigabytes(have as f64)
    ))
}

fn timing_of(kind: tc::TimestampKind) -> Timing {
    match kind {
        tc::TimestampKind::Word | tc::TimestampKind::Token => Timing::Words,
        tc::TimestampKind::Segment => Timing::Phrases,
        _ => Timing::Pauses,
    }
}

fn secs(samples: i64) -> f64 {
    samples as f64 / f64::from(RATE)
}

fn within(stretches: &[(i64, i64)], from: f64, to: f64) -> Vec<(f64, f64)> {
    stretches
        .iter()
        .map(|&(s, e)| (secs(s).max(from), secs(e).min(to)))
        .filter(|(s, e)| e > s)
        .collect()
}

fn said_of(r: &tc::Transcript, piece: (i64, i64), stretches: &[(i64, i64)]) -> Vec<Said> {
    let at = secs(piece.0);
    let ms = |t: i64| at + t as f64 / 1000.0;
    let word = |w: &tc::Word| Word {
        start: ms(w.t0_ms),
        end: ms(w.t1_ms),
        text: w.text.trim().to_string(),
        joined: false,
        speaker: None,
    };
    if !r.words.is_empty() {
        if r.segments.is_empty() {
            return vec![Said {
                start: secs(piece.0),
                end: secs(piece.1),
                text: r.text.trim().to_string(),
                words: r.words.iter().map(word).collect(),
                speech: Vec::new(),
            }];
        }
        return r
            .segments
            .iter()
            .enumerate()
            .map(|(i, s)| Said {
                start: ms(s.t0_ms),
                end: ms(s.t1_ms),
                text: s.text.trim().to_string(),
                words: r
                    .words
                    .iter()
                    .filter(|w| w.seg_index == i as i32)
                    .map(word)
                    .collect(),
                speech: Vec::new(),
            })
            .collect();
    }
    if r.timestamp_kind == tc::TimestampKind::Segment && !r.segments.is_empty() {
        return r
            .segments
            .iter()
            .map(|s| {
                let (from, to) = (ms(s.t0_ms), ms(s.t1_ms));
                Said {
                    start: from,
                    end: to,
                    text: s.text.trim().to_string(),
                    words: Vec::new(),
                    speech: within(stretches, from, to),
                }
            })
            .collect();
    }
    let (from, to) = (secs(piece.0), secs(piece.1));
    vec![Said {
        start: from,
        end: to,
        text: r.text.trim().to_string(),
        words: Vec::new(),
        speech: within(stretches, from, to),
    }]
}

pub fn transcribe_files(
    plans: &[&RenderPlan],
    asked: &Asked,
    options: &Options,
) -> Result<Vec<Speech>> {
    match asked.model.paid() && asked.model_file.is_none() {
        true => crate::speech_api::transcribe(plans, asked, options),
        false => plans
            .iter()
            .map(|p| transcribe(p, asked, options))
            .collect(),
    }
}

pub fn transcribe(plan: &RenderPlan, asked: &Asked, options: &Options) -> Result<Speech> {
    if asked.model.paid() && asked.model_file.is_none() {
        return crate::speech_api::transcribe(&[plan], asked, options)?
            .pop()
            .ok_or_else(|| Error::Invalid("no transcript came back".into()));
    }
    let started = Instant::now();
    let mut telling = Telling::here(asked);
    let (model_path, vad_path) = files(asked, options.models)?;
    let pcm = heard_plan(plan, &model_path, started)?;
    if plan.channels != 1 {
        return Err(Error::Invalid(format!(
            "speech is heard in one channel, not {}",
            plan.channels
        )));
    }
    let mut warnings = Vec::new();
    let Found {
        stretches,
        quiet,
        frame,
        found,
        floor,
    } = find_speech(&vad_path, &pcm, asked, &mut warnings)?;
    report(&model_path, Stage::Loading, 0, 0, started);
    let loaded = load(&model_path, options.gpu, &mut warnings)?;
    crate::cancel::stop()?;
    telling.tell(|n| n.device = Some(loaded.device.clone()));
    if loaded.gpu
        && asked.model_file.is_none()
        && let Some(warning) = file_of(asked.model).and_then(short_of_memory)
    {
        warnings.push(warning);
    }
    let caps = loaded.model.capabilities().map_err(tc_error)?;
    if let Some(l) = &asked.language
        && !caps.languages.is_empty()
        && !caps.languages.iter().any(|k| k.eq_ignore_ascii_case(l))
    {
        return Err(Error::Invalid(format!(
            "{} doesn't know the language {l:?}; it knows {}",
            model_path.display(),
            caps.languages.join(", ")
        )));
    }
    let longest = longest_piece(
        &loaded.model.arch(),
        timing_of(caps.max_timestamp_kind) != Timing::Pauses,
        loaded.gpu,
        loaded.memory_mb,
        caps.max_audio_ms,
    );
    let pieces: Vec<(i64, i64)> = transcript::pieces(
        &stretches,
        &quiet,
        frame,
        (longest * f64::from(RATE)) as i64,
        (PAD * f64::from(RATE)) as i64,
    )
    .into_iter()
    .map(|(s, e)| (s.max(0), e.min(pcm.len() as i64)))
    .filter(|(s, e)| e > s)
    .collect();
    let mut order: Vec<usize> = (0..pieces.len()).collect();
    if asked.language.is_none()
        && let Some(first) = (0..pieces.len()).max_by_key(|&i| pieces[i].1 - pieces[i].0)
    {
        order.retain(|&i| i != first);
        order.insert(0, first);
    }
    let mut session = loaded.model.session().map_err(tc_error)?;
    let mut language = asked.language.clone();
    let total: i64 = pieces.iter().map(|(s, e)| e - s).sum();
    let mut done = 0;
    let mut heard: Vec<Option<tc::Transcript>> = vec![None; pieces.len()];
    for &i in &order {
        crate::cancel::stop()?;
        let (s, e) = pieces[i];
        let run = tc::RunOptions {
            language: language.clone(),
            ..Default::default()
        };
        let r = match session.run(&pcm[s as usize..e as usize], &run) {
            Ok(r) => r,
            Err(err) => match err.partial().cloned() {
                Some(partial) if !matches!(err, tc::Error::Aborted { .. }) => {
                    warnings.push(format!(
                        "{}–{}: {err}; kept what was transcribed",
                        transcript_clock(secs(s)),
                        transcript_clock(secs(e))
                    ));
                    partial
                }
                _ => return Err(tc_error(err)),
            },
        };
        if language.is_none() {
            language = r.language.clone();
            if let Some(l) = &language {
                telling.heard(l, asked.speaker_model);
            }
        }
        heard[i] = Some(r);
        done += e - s;
        report(
            &model_path,
            Stage::Transcribing,
            done * 1000 / i64::from(RATE),
            total * 1000 / i64::from(RATE),
            started,
        );
    }
    let mut said: Vec<Said> = heard
        .iter()
        .zip(&pieces)
        .filter_map(|(r, &piece)| Some(said_of(r.as_ref()?, piece, &stretches)))
        .flatten()
        .filter(|s| !s.text.is_empty() || !s.words.is_empty())
        .collect();
    drop(session);
    let device = loaded.device.clone();
    drop(loaded);
    let speakers = match asked.speakers {
        true => {
            let heard_language = language.clone();
            let at = Hearing {
                dir: options.models,
                gpu: options.gpu,
                download: asked.download,
                started,
            };
            let (turns, by) = speakers_of(
                &at,
                &pcm,
                heard_language.as_deref(),
                asked.speaker_model,
                &mut telling,
                &mut warnings,
            )?;
            transcript::give_speakers(&mut said, &turns);
            Some((run_id(), by))
        }
        false => None,
    };
    Ok(Speech {
        model: asked.model_name(),
        device,
        language,
        timing: timing_of(caps.max_timestamp_kind),
        pieces: pieces.len(),
        longest_piece: pieces.iter().map(|(s, e)| secs(e - s)).fold(0.0, f64::max),
        said,
        warnings,
        found,
        sound_db: (found == FindSpeech::Sound).then_some(floor),
        speakers_by: speakers.as_ref().map(|(_, by)| by.to_string()),
        speakers: speakers.map(|(run, _)| run),
        notes: Vec::new(),
    })
}

fn transcript_clock(secs: f64) -> String {
    vidcut_core::subtitles::clock(secs, '.')
}

#[cfg(test)]
mod tests {
    use super::full;
    use vidcut_core::transcript::Turn;

    fn turn(speaker: u32) -> Turn {
        Turn {
            start: 0.0,
            end: 1.0,
            speaker,
        }
    }

    #[test]
    fn four_speakers_say_more_may_share_numbers() {
        assert!(full(&[turn(1), turn(2), turn(3), turn(4)], 4, false).is_some());
    }

    #[test]
    fn three_speakers_say_nothing() {
        assert_eq!(full(&[turn(1), turn(2), turn(3), turn(1)], 4, false), None);
    }

    #[test]
    fn english_and_chinese_take_moss_for_speakers() {
        assert_eq!(
            super::speaker_file(Some("en"), Default::default()).name,
            super::MOSS.name
        );
    }

    #[test]
    fn finnish_takes_sortformer_for_speakers() {
        assert_eq!(
            super::speaker_file(Some("fi"), Default::default()).name,
            super::SORTFORMER.name
        );
    }

    #[test]
    fn an_unheard_language_names_no_speaker_model_yet() {
        assert_eq!(
            super::speakers_by(None, Default::default()).map(|f| f.name),
            None
        );
    }

    #[test]
    fn an_existing_library_is_not_unpacked_again() {
        let Some(runtime) = super::RUNTIME else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let home = runtime.home(dir.path());
        std::fs::create_dir_all(&home).unwrap();
        let library = home.join(runtime.library);
        std::fs::write(&library, b"kept").unwrap();
        super::unpack(&dir.path().join("missing.tar.gz"), &runtime, dir.path()).unwrap();
        assert_eq!(std::fs::read(&library).unwrap(), b"kept");
    }

    #[test]
    fn english_asked_takes_moss_before_it_is_downloaded() {
        assert_eq!(
            super::speakers_by(Some("en"), Default::default()).map(|f| f.name),
            Some(super::MOSS.name)
        );
    }
}
