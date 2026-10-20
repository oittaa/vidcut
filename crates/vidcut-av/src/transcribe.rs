use std::ffi::{CString, c_int, c_void};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use sha2::{Digest, Sha256};
use transcribe_cpp as tc;
use vidcut_core::analysis::VoiceActivity;
use vidcut_core::model::Media;
use vidcut_core::render::{AudioPiece, RenderPlan};
use vidcut_core::request::{FindSpeech, SpeechModel};
use vidcut_core::transcript::{self, Asked, RATE, Said, Speech, Timing, Word};

use crate::error::{Error, Result};
use crate::export::audio::AudioMix;
use crate::export::{CANCELLED, DECLICK_MS, Progress, Stage};
use crate::ffmpeg::ffi;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct File {
    pub name: &'static str,
    pub title: &'static str,
    pub repo: &'static str,
    pub revision: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    pub gpu_gb: u64,
    pub languages: &'static [&'static str],
}

impl File {
    pub fn url(&self) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.repo, self.revision, self.name
        )
    }
}

pub const QWEN3_ASR: File = File {
    name: "Qwen3-ASR-1.7B-Q8_0.gguf",
    title: SpeechModel::Qwen3Asr.title(),
    repo: "handy-computer/Qwen3-ASR-1.7B-gguf",
    revision: "0e0a596e3959c3f3f2ab7d443a5b6f0f4a4bba6f",
    bytes: 2_185_030_624,
    sha256: "9a0d81792dfea2d5f278b8a63deb3ea6e02139ce42c2301f32ea19c4f77526b7",
    gpu_gb: 8,
    languages: &[
        "zh", "en", "yue", "ar", "de", "fr", "es", "pt", "id", "it", "ko", "ru", "th", "vi", "ja",
        "tr", "hi", "ms", "nl", "sv", "da", "fi", "pl", "cs", "fil", "fa", "el", "ro", "hu", "mk",
    ],
};

pub const WHISPER_LARGE_V3: File = File {
    name: "whisper-large-v3-Q8_0.gguf",
    title: SpeechModel::WhisperLargeV3.title(),
    repo: "handy-computer/whisper-large-v3-gguf",
    revision: "274ad1d1cf680b7ee53cc70f78461394cd3a344b",
    bytes: 1_668_741_440,
    sha256: "2fa1a5f179f8a511a53e2108db270aa4af3ce08cd976af4180e2854666bb4ba3",
    gpu_gb: 4,
    languages: &[
        "af", "am", "ar", "as", "az", "ba", "be", "bg", "bn", "bo", "br", "bs", "ca", "cs", "cy",
        "da", "de", "el", "en", "es", "et", "eu", "fa", "fi", "fo", "fr", "gl", "gu", "haw", "ha",
        "he", "hi", "hr", "ht", "hu", "hy", "id", "is", "it", "ja", "jw", "ka", "kk", "km", "kn",
        "ko", "la", "lb", "ln", "lo", "lt", "lv", "mg", "mi", "mk", "ml", "mn", "mr", "ms", "mt",
        "my", "ne", "nl", "nn", "no", "oc", "pa", "pl", "ps", "pt", "ro", "ru", "sa", "sd", "si",
        "sk", "sl", "sn", "so", "sq", "sr", "su", "sv", "sw", "ta", "te", "tg", "th", "tk", "tl",
        "tr", "tt", "uk", "ur", "uz", "vi", "yi", "yo", "yue", "zh",
    ],
};

pub const PARAKEET_ULTRA: File = File {
    name: "parakeet-ultra-Q8_0.gguf",
    title: SpeechModel::ParakeetUltra.title(),
    repo: "handy-computer/parakeet-ultra-gguf",
    revision: "a1539be1b849ea7463b88343e5277b373d0c4ea9",
    bytes: 740_363_168,
    sha256: "007a59761e9258779f189df396b50d12b83bb5b79e66df4b955c230d2f2a0a59",
    gpu_gb: 2,
    languages: &[
        "en", "de", "fr", "es", "it", "pt", "ru", "uk", "hr", "sl", "lv", "lt", "et", "fi", "sv",
        "da", "nl", "pl", "cs", "sk", "hu", "ro", "bg", "el", "mt",
    ],
};

pub const SILERO_VAD: File = File {
    name: "silero-vad-v6.2-F32.gguf",
    title: "Silero VAD v6.2",
    repo: "handy-computer/silero-vad-v6.2-gguf",
    revision: "a9b2d61c20ae4ddf3644edb34969e8185a485448",
    bytes: 1_241_056,
    sha256: "0d871a800c420fdd5d1b17a3aea4296d1c39608f0ff4a9295eb76457eb1c88e6",
    gpu_gb: 0,
    languages: &[],
};

const TRANSCRIBE: &str = "0.4.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Runtime {
    pub lane: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    pub library: &'static str,
}

impl Runtime {
    fn archive(&self) -> String {
        format!("transcribe-native-{TRANSCRIBE}-{}.tar.gz", self.lane)
    }

    fn url(&self) -> String {
        format!(
            "https://github.com/handy-computer/transcribe.cpp/releases/download/v{TRANSCRIBE}/{}",
            self.archive()
        )
    }

    fn home(&self, dir: &Path) -> PathBuf {
        dir.join(format!("transcribe-{TRANSCRIBE}"))
    }

    fn present(&self, dir: &Path) -> bool {
        self.home(dir).join(self.library).is_file()
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
pub const RUNTIME: Option<Runtime> = Some(Runtime {
    lane: "windows-x86_64-cpu-vulkan",
    bytes: 18_036_555,
    sha256: "9d5bd89f720d7979323e73f5220fe50fad5e38a7b0c2525225d453bd09adf2b0",
    library: "transcribe.dll",
});

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub const RUNTIME: Option<Runtime> = Some(Runtime {
    lane: "macos-arm64-metal",
    bytes: 2_475_842,
    sha256: "eeae15ef08cfbfdd059238765a1a0f790dd39e2f0fdb05bfbb601e183286f6d7",
    library: "libtranscribe.dylib",
});

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub const RUNTIME: Option<Runtime> = Some(Runtime {
    lane: "macos-x86_64-cpu",
    bytes: 1_600_612,
    sha256: "e4da34c9cebd8b31ca2c1049296430ff7af1a851db436e23e08d3d66e09b7d0f",
    library: "libtranscribe.dylib",
});

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub const RUNTIME: Option<Runtime> = Some(Runtime {
    lane: "linux-x86_64-cpu-vulkan",
    bytes: 22_546_161,
    sha256: "f229adb9018bb7e33042137f92942022a5d76bfc5e16efd7a11cc9b115e86ea2",
    library: "libtranscribe.so",
});

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub const RUNTIME: Option<Runtime> = Some(Runtime {
    lane: "linux-aarch64-cpu-vulkan",
    bytes: 18_122_209,
    sha256: "af7ee108923b74d3e727309ce4992714a25872ea77c6247da8be9a0ed54275dd",
    library: "libtranscribe.so",
});

#[cfg(not(any(
    all(windows, target_arch = "x86_64"),
    all(
        target_os = "macos",
        any(target_arch = "aarch64", target_arch = "x86_64")
    ),
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
)))]
pub const RUNTIME: Option<Runtime> = None;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Need {
    pub title: String,
    pub bytes: u64,
}

fn needed_for(dir: &Path, model: Option<&File>, vad: bool) -> Vec<Need> {
    let mut out = Vec::new();
    if let Some(r) = RUNTIME.filter(|r| !r.present(dir)) {
        out.push(Need {
            title: "transcribe.cpp's library".into(),
            bytes: r.bytes,
        });
    }
    let files = vad.then_some(&SILERO_VAD).into_iter().chain(model);
    for f in files.filter(|f| !present(dir, f)) {
        out.push(Need {
            title: f.title.into(),
            bytes: f.bytes,
        });
    }
    out
}

pub fn needed(dir: &Path, model: Option<&File>) -> Vec<Need> {
    needed_for(dir, model, true)
}

pub fn listed(needs: &[Need]) -> String {
    let names: Vec<String> = needs
        .iter()
        .map(|n| format!("{} ({})", n.title, size(n.bytes)))
        .collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

pub fn file_of(model: SpeechModel) -> &'static File {
    match model {
        SpeechModel::Qwen3Asr => &QWEN3_ASR,
        SpeechModel::WhisperLargeV3 => &WHISPER_LARGE_V3,
        SpeechModel::ParakeetUltra => &PARAKEET_ULTRA,
    }
}

pub struct Options<'a> {
    pub models: &'a Path,
    pub gpu: bool,
}

const PAD: f64 = 0.2;
const UNTIMED: f64 = 10.0;
const SHORTEST: f64 = 30.0;
const LONGEST: f64 = 300.0;
const QWEN3_BASE_MB: f64 = 3300.0;
const QWEN3_MB_PER_SECOND: f64 = 21.0;

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

fn network() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        crate::init();
        // SAFETY: FFmpeg's network setup, called once before any network protocol is used.
        unsafe { ffi::avformat_network_init() };
    });
}

fn start(dir: &Path, runtime: &Runtime) -> Result<()> {
    static STARTED: Mutex<bool> = Mutex::new(false);
    let mut started = STARTED.lock().unwrap_or_else(PoisonError::into_inner);
    if *started {
        return Ok(());
    }
    let home = runtime.home(dir);
    let longest = std::fs::read_dir(&home)?
        .filter_map(|e| e.ok())
        .map(|e| e.path().as_os_str().len())
        .max()
        .unwrap_or(0);
    if cfg!(windows) && longest >= 260 {
        return Err(Error::Invalid(format!(
            "transcribe.cpp's library is in {}, where its files' paths pass the 260 characters \
             Windows loads from: set XDG_CACHE_HOME to a shorter folder",
            home.display()
        )));
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

fn get_ready(dir: &Path, model: Option<&File>, vad: bool, download: bool) -> Result<()> {
    network();
    let Some(runtime) = RUNTIME else {
        return Err(Error::Missing(
            "transcribe.cpp publishes no library for this system, so speech can't be heard here"
                .into(),
        ));
    };
    let needs = needed_for(dir, model, vad);
    if !needs.is_empty() && !download {
        let own = match model.is_some_and(|f| !present(dir, f)) {
            true => ", or name a model file of your own with model_file",
            false => "",
        };
        let total: u64 = needs.iter().map(|n| n.bytes).sum();
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
    for f in vad.then_some(&SILERO_VAD).into_iter().chain(model) {
        if !present(dir, f) {
            fetch(&Fetch::of(f), dir)?;
        }
    }
    start(dir, &runtime)
}

fn unpack(archive: &Path, runtime: &Runtime, dir: &Path) -> Result<()> {
    let home = runtime.home(dir);
    let part = dir.join(format!("transcribe-{TRANSCRIBE}.part"));
    let _ = std::fs::remove_dir_all(&part);
    std::fs::create_dir_all(&part)?;
    let gz = flate2::read::GzDecoder::new(std::fs::File::open(archive)?);
    tar::Archive::new(gz).unpack(&part)?;
    let inner = std::iter::once(part.clone())
        .chain(
            std::fs::read_dir(&part)?
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
    std::fs::rename(&inner, &home)?;
    let _ = std::fs::remove_dir_all(&part);
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

pub fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1_000_000_000 => format!("{:.1} GB", b as f64 / 1e9),
        b if b >= 1_000_000 => format!("{:.0} MB", b as f64 / 1e6),
        b => format!("{:.0} kB", b as f64 / 1e3),
    }
}

pub fn present(dir: &Path, f: &File) -> bool {
    std::fs::metadata(dir.join(f.name)).is_ok_and(|m| m.len() == f.bytes)
}

fn files(asked: &Asked, dir: &Path) -> Result<(PathBuf, PathBuf)> {
    let wanted = asked.model_file.is_none().then(|| file_of(asked.model));
    get_ready(dir, wanted, asked.vad_file.is_none(), asked.download)?;
    let model = asked
        .model_file
        .clone()
        .unwrap_or_else(|| dir.join(file_of(asked.model).name));
    let vad = asked
        .vad_file
        .clone()
        .unwrap_or_else(|| dir.join(SILERO_VAD.name));
    for path in [&model, &vad] {
        if !path.is_file() {
            return Err(Error::Missing(format!("{}: no such file", path.display())));
        }
    }
    Ok((model, vad))
}

unsafe extern "C" fn interrupted(opaque: *mut c_void) -> c_int {
    // SAFETY: opaque is the AtomicBool that download() keeps alive while the connection is open.
    let flag = unsafe { &*(opaque as *const AtomicBool) };
    c_int::from(flag.load(Ordering::Relaxed))
}

struct Connection(*mut ffi::AVIOContext);

impl Drop for Connection {
    fn drop(&mut self) {
        // SAFETY: a context avio_open2 made, closed once.
        unsafe { ffi::avio_closep(&mut self.0) };
    }
}

fn ffmpeg_error(context: String, code: c_int) -> Error {
    Error::Ffmpeg {
        context,
        source: crate::ffmpeg::Error::from(code),
    }
}

fn open(url: &str, stop: &Arc<AtomicBool>) -> Result<Connection> {
    let c_url = CString::new(url).map_err(|e| Error::Invalid(e.to_string()))?;
    let callback = ffi::AVIOInterruptCB {
        callback: Some(interrupted),
        opaque: Arc::as_ptr(stop) as *mut c_void,
    };
    let mut options: *mut ffi::AVDictionary = std::ptr::null_mut();
    let mut ctx: *mut ffi::AVIOContext = std::ptr::null_mut();
    // SAFETY: FFmpeg's documented calls; every pointer outlives them, and the dictionary is freed here.
    let opened = unsafe {
        ffi::av_dict_set(
            &mut options,
            c"rw_timeout".as_ptr(),
            c"30000000".as_ptr(),
            0,
        );
        let r = ffi::avio_open2(
            &mut ctx,
            c_url.as_ptr(),
            ffi::AVIO_FLAG_READ,
            &callback,
            &mut options,
        );
        ffi::av_dict_free(&mut options);
        r
    };
    if opened == ffi::AVERROR_PROTOCOL_NOT_FOUND {
        return Err(Error::Missing(format!(
            "this FFmpeg can't download over HTTPS: download {url} yourself"
        )));
    }
    if opened < 0 {
        return Err(ffmpeg_error(format!("downloading {url}"), opened));
    }
    Ok(Connection(ctx))
}

struct Fetch {
    url: String,
    name: String,
    title: String,
    bytes: u64,
    sha256: &'static str,
}

impl Fetch {
    fn of(f: &File) -> Self {
        Self {
            url: f.url(),
            name: f.name.to_string(),
            title: f.title.to_string(),
            bytes: f.bytes,
            sha256: f.sha256,
        }
    }
}

fn fetch(f: &Fetch, dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    crate::progress::note(crate::export::Note::Downloading {
        name: format!("{} ({})", f.title, size(f.bytes)),
    });
    let target = dir.join(&f.name);
    let part = dir.join(format!("{}.part", f.name));
    let stop = crate::cancel::current().unwrap_or_default();
    let started = Instant::now();
    let url = f.url.clone();
    let mut hash = Sha256::new();
    let mut have = 0u64;
    if let Ok(mut old) = std::fs::File::open(&part)
        && old.metadata().is_ok_and(|m| m.len() < f.bytes)
    {
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = old.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
            have += n as u64;
        }
    }
    let connection = open(&url, &stop)?;
    if have > 0 {
        // SAFETY: an open context; FFmpeg's HTTP protocol asks the server for the rest.
        let at = unsafe { ffi::avio_seek(connection.0, have as i64, libc::SEEK_SET) };
        if at != have as i64 {
            (hash, have) = (Sha256::new(), 0);
            drop(connection);
            download_from(f, &target, &part, open(&url, &stop)?, hash, have, started)?;
            return Ok(target);
        }
    }
    download_from(f, &target, &part, connection, hash, have, started)?;
    Ok(target)
}

fn download_from(
    f: &Fetch,
    target: &Path,
    part: &Path,
    connection: Connection,
    mut hash: Sha256,
    mut have: u64,
    started: Instant,
) -> Result<()> {
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(have > 0)
        .write(true)
        .truncate(have == 0)
        .open(part)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if crate::cancel::cancelled() {
            return Err(Error::Invalid(CANCELLED.into()));
        }
        // SAFETY: an open context and a buffer of the length given.
        let n = unsafe { ffi::avio_read(connection.0, buf.as_mut_ptr(), buf.len() as c_int) };
        if n == ffi::AVERROR_EOF {
            break;
        }
        if n < 0 {
            if crate::cancel::cancelled() {
                return Err(Error::Invalid(CANCELLED.into()));
            }
            return Err(ffmpeg_error(format!("downloading {}", f.url), n));
        }
        let n = n as usize;
        out.write_all(&buf[..n])?;
        hash.update(&buf[..n]);
        have += n as u64;
        if have > f.bytes {
            break;
        }
        report(
            target,
            Stage::Downloading,
            have as i64,
            f.bytes as i64,
            started,
        );
    }
    out.flush()?;
    drop(out);
    let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if have != f.bytes || digest != f.sha256 {
        let _ = std::fs::remove_file(part);
        return Err(Error::Invalid(format!(
            "{}: downloaded {have} bytes with SHA-256 {digest}, expected {} bytes with {}",
            f.url, f.bytes, f.sha256
        )));
    }
    std::fs::rename(part, target)?;
    Ok(())
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

fn heard_plan(plan: &RenderPlan, path: &Path, started: Instant) -> Result<Vec<f32>> {
    if plan.sample_rate != RATE || plan.channels != 1 {
        return Err(Error::Invalid(format!(
            "speech is heard at {RATE} Hz in one channel, not {} Hz in {}",
            plan.sample_rate, plan.channels
        )));
    }
    let total = plan.samples().max(0);
    let mut pcm = Vec::with_capacity(total as usize);
    listen(&plan.audio, total, path, started, |s| {
        pcm.extend_from_slice(s);
        Ok(())
    })?;
    Ok(pcm)
}

fn vad_model(vad: &Path) -> Result<tc::Model> {
    tc::Model::load_with(
        vad,
        &tc::ModelOptions {
            backend: tc::Backend::Cpu,
            device: None,
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
    get_ready(models, None, true, download)?;
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
        fade: None,
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
                let label = format!("{} ({})", device.description, kind_name(&device.kind));
                match tc::Model::load_with(
                    path,
                    &tc::ModelOptions {
                        backend,
                        device: Some(device),
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

pub fn short_of_memory(f: &File) -> Option<String> {
    let have = crate::gpus::in_use()?.memory_mb?;
    (have < f.gpu_gb * 1024).then(|| {
        format!(
            "{} needs about {} GB of GPU memory; this GPU has {} GB",
            f.title,
            f.gpu_gb,
            gigabytes(have as f64)
        )
    })
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

pub fn transcribe(plan: &RenderPlan, asked: &Asked, options: &Options) -> Result<Speech> {
    let started = Instant::now();
    let (model_path, vad_path) = files(asked, options.models)?;
    let pcm = heard_plan(plan, &model_path, started)?;
    let (voice, frame) = speech(&vad_path, &pcm)?;
    let quiet: Vec<f32> = pcm
        .chunks(frame)
        .map(|c| (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    let mut warnings = Vec::new();
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
    };
    report(&model_path, Stage::Loading, 0, 0, started);
    let loaded = load(&model_path, options.gpu, &mut warnings)?;
    crate::cancel::stop()?;
    if loaded.gpu
        && asked.model_file.is_none()
        && let Some(warning) = short_of_memory(file_of(asked.model))
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
    let said: Vec<Said> = heard
        .iter()
        .zip(&pieces)
        .filter_map(|(r, &piece)| Some(said_of(r.as_ref()?, piece, &stretches)))
        .flatten()
        .filter(|s| !s.text.is_empty() || !s.words.is_empty())
        .collect();
    Ok(Speech {
        model: asked.model_name(),
        device: loaded.device,
        language,
        timing: timing_of(caps.max_timestamp_kind),
        pieces: pieces.len(),
        longest_piece: pieces.iter().map(|(s, e)| secs(e - s)).fold(0.0, f64::max),
        said,
        warnings,
        found,
        sound_db: (found == FindSpeech::Sound).then_some(floor),
    })
}

fn transcript_clock(secs: f64) -> String {
    vidcut_core::subtitles::clock(secs, '.')
}
