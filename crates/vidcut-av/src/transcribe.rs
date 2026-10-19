use std::ffi::{CString, c_int, c_void};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use sha2::{Digest, Sha256};
use transcribe_cpp as tc;
use vidcut_core::render::RenderPlan;
use vidcut_core::request::SpeechModel;
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
    title: "Qwen3-ASR 1.7B",
    repo: "handy-computer/Qwen3-ASR-1.7B-gguf",
    revision: "0e0a596e3959c3f3f2ab7d443a5b6f0f4a4bba6f",
    bytes: 2_185_030_624,
    sha256: "9a0d81792dfea2d5f278b8a63deb3ea6e02139ce42c2301f32ea19c4f77526b7",
};

pub const WHISPER_LARGE_V3: File = File {
    name: "whisper-large-v3-Q8_0.gguf",
    title: "Whisper large-v3",
    repo: "handy-computer/whisper-large-v3-gguf",
    revision: "274ad1d1cf680b7ee53cc70f78461394cd3a344b",
    bytes: 1_668_741_440,
    sha256: "2fa1a5f179f8a511a53e2108db270aa4af3ce08cd976af4180e2854666bb4ba3",
};

pub const PARAKEET_ULTRA: File = File {
    name: "parakeet-ultra-Q8_0.gguf",
    title: "Parakeet Ultra",
    repo: "handy-computer/parakeet-ultra-gguf",
    revision: "a1539be1b849ea7463b88343e5277b373d0c4ea9",
    bytes: 740_363_168,
    sha256: "007a59761e9258779f189df396b50d12b83bb5b79e66df4b955c230d2f2a0a59",
};

pub const SILERO_VAD: File = File {
    name: "silero-vad-v6.2-F32.gguf",
    title: "the Silero VAD model",
    repo: "handy-computer/silero-vad-v6.2-gguf",
    revision: "a9b2d61c20ae4ddf3644edb34969e8185a485448",
    bytes: 1_241_056,
    sha256: "0d871a800c420fdd5d1b17a3aea4296d1c39608f0ff4a9295eb76457eb1c88e6",
};

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

fn init() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        crate::init();
        // SAFETY: FFmpeg's network setup, called once before any network protocol is used.
        unsafe { ffi::avformat_network_init() };
        if !crate::debug() {
            tc::disable_logging();
        }
        if !vulkan_loader() {
            let _ = tc::init_backends_with(None::<&Path>, tc::BackendMask::CPU);
        }
    });
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

fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1_000_000_000 => format!("{:.1} GB", b as f64 / 1e9),
        b if b >= 1_000_000 => format!("{:.0} MB", b as f64 / 1e6),
        b => format!("{:.0} kB", b as f64 / 1e3),
    }
}

fn present(dir: &Path, f: &File) -> bool {
    std::fs::metadata(dir.join(f.name)).is_ok_and(|m| m.len() == f.bytes)
}

pub fn missing(asked: &Asked, dir: &Path) -> Vec<&'static File> {
    let mut wanted = Vec::new();
    if asked.model_file.is_none() && !present(dir, file_of(asked.model)) {
        wanted.push(file_of(asked.model));
    }
    if asked.vad_file.is_none() && !present(dir, &SILERO_VAD) {
        wanted.push(&SILERO_VAD);
    }
    wanted
}

fn files(asked: &Asked, dir: &Path) -> Result<(PathBuf, PathBuf)> {
    let wanted = missing(asked, dir);
    if !wanted.is_empty() && !asked.download {
        let list: Vec<String> = wanted
            .iter()
            .map(|f| format!("{} ({}, {})", f.title, f.name, size(f.bytes)))
            .collect();
        let (verb, them) = match wanted.len() {
            1 => ("isn't", "it"),
            _ => ("aren't", "them"),
        };
        return Err(Error::Missing(format!(
            "{} {verb} on this computer yet: pass download (--download) to download {them} from \
             Hugging Face into {}, checked against SHA-256, or name files of your own with \
             model_file and vad_file",
            list.join(" and "),
            dir.display()
        )));
    }
    for f in wanted {
        download(f, dir)?;
    }
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
            "this FFmpeg can't download over HTTPS: download {url} yourself and name the file \
             with model_file or vad_file"
        )));
    }
    if opened < 0 {
        return Err(ffmpeg_error(format!("downloading {url}"), opened));
    }
    Ok(Connection(ctx))
}

fn download(f: &File, dir: &Path) -> Result<()> {
    init();
    std::fs::create_dir_all(dir)?;
    let target = dir.join(f.name);
    let part = dir.join(format!("{}.part", f.name));
    let stop = crate::cancel::current().unwrap_or_default();
    let started = Instant::now();
    let url = f.url();
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
            return download_from(f, &target, &part, open(&url, &stop)?, hash, have, started);
        }
    }
    download_from(f, &target, &part, connection, hash, have, started)
}

fn download_from(
    f: &File,
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
            return Err(ffmpeg_error(format!("downloading {}", f.url()), n));
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
            f.url(),
            f.bytes,
            f.sha256
        )));
    }
    std::fs::rename(part, target)?;
    Ok(())
}

fn listen(plan: &RenderPlan, path: &Path, started: Instant) -> Result<Vec<f32>> {
    if plan.sample_rate != RATE || plan.channels != 1 {
        return Err(Error::Invalid(format!(
            "speech is heard at {RATE} Hz in one channel, not {} Hz in {}",
            plan.sample_rate, plan.channels
        )));
    }
    let total = plan.samples().max(0);
    let mut mix = AudioMix::new(&plan.audio, RATE, 1, DECLICK_MS);
    let mut pcm = Vec::with_capacity(total as usize);
    while (pcm.len() as i64) < total {
        crate::cancel::stop()?;
        let n = (total - pcm.len() as i64).min(i64::from(RATE) * 10) as usize;
        pcm.extend(mix.read(n)?);
        let ms = |s: i64| s * 1000 / i64::from(RATE);
        report(
            path,
            Stage::Listening,
            ms(pcm.len() as i64),
            ms(total),
            started,
        );
    }
    Ok(pcm)
}

fn speech(vad: &Path, pcm: &[f32]) -> Result<(Vec<(i64, i64)>, usize)> {
    let model = tc::Model::load_with(
        vad,
        &tc::ModelOptions {
            backend: tc::Backend::Cpu,
            device: None,
        },
    )
    .map_err(tc_error)?;
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
    init();
    let started = Instant::now();
    let (model_path, vad_path) = files(asked, options.models)?;
    let pcm = listen(plan, &model_path, started)?;
    let (stretches, frame) = speech(&vad_path, &pcm)?;
    let quiet: Vec<f32> = pcm
        .chunks(frame)
        .map(|c| (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    let mut warnings = Vec::new();
    let loaded = load(&model_path, options.gpu, &mut warnings)?;
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
    let model = match &asked.model_file {
        Some(f) => f.file_name().map_or_else(
            || f.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        ),
        None => file_of(asked.model).title.to_string(),
    };
    Ok(Speech {
        model,
        device: loaded.device,
        language,
        timing: timing_of(caps.max_timestamp_kind),
        pieces: pieces.len(),
        longest_piece: pieces.iter().map(|(s, e)| secs(e - s)).fold(0.0, f64::max),
        said,
        warnings,
    })
}

fn transcript_clock(secs: f64) -> String {
    vidcut_core::subtitles::clock(secs, '.')
}
