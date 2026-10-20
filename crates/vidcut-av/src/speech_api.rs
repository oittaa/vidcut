use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use vidcut_core::render::RenderPlan;
use vidcut_core::request::{FindSpeech, SpeechModel};
use vidcut_core::transcript::{self, Asked, RATE, Said, Speech, Timing, Word};

use crate::error::{Context, Error, Result};
use crate::export::{CANCELLED, Progress, Stage};
use crate::ffmpeg::{self, codec, encoder, format, frame};
use crate::transcribe::{self, Options};

const XAI_URL: &str = "https://api.x.ai/v1/stt";
const XAI_MODEL: &str = "grok-voice-transcribe-2.0";
const XAI_LIMIT: u64 = 500_000_000;
const GOOGLE: &str = "https://generativelanguage.googleapis.com";
const GOOGLE_MODEL: &str = "gemini-3.5-transcribe";
const GOOGLE_LONGEST: f64 = 30.0 * 60.0;
const PAD: f64 = 0.2;
const GAP: f64 = 0.5;
const WAIT: Duration = Duration::from_secs(60 * 60);

const GOOGLE_LANGUAGES: [&str; 83] = [
    "en-US",
    "es-US",
    "pt-BR",
    "cmn-Hans-CN",
    "yue-Hant-HK",
    "bn-BD",
    "pa-IN",
    "af-ZA",
    "am-ET",
    "ar-EG",
    "hy-AM",
    "as-IN",
    "az-AZ",
    "be-BY",
    "bn-IN",
    "bs-BA",
    "bg-BG",
    "rup-BG",
    "my-MM",
    "ca-ES",
    "ceb",
    "km-KH",
    "hr-HR",
    "cs-CZ",
    "da-DK",
    "nl-NL",
    "en-GB",
    "en-IN",
    "et-EE",
    "fa-IR",
    "fil-PH",
    "fi-FI",
    "fr-FR",
    "gl-ES",
    "ka-GE",
    "de-DE",
    "el-GR",
    "gu-IN",
    "ha-NG",
    "he-IL",
    "hi-IN",
    "hu-HU",
    "is-IS",
    "id-ID",
    "it-IT",
    "ja-JP",
    "jv-ID",
    "kea-CV",
    "kn-IN",
    "kk-KZ",
    "ko-KR",
    "ky-KG",
    "lv-LV",
    "ln-CD",
    "lt-LT",
    "mk-MK",
    "ms-MY",
    "ml-IN",
    "mt-MT",
    "mr-IN",
    "mn-MN",
    "ne-NP",
    "nb-NO",
    "or-IN",
    "pl-PL",
    "pt-PT",
    "pa-Guru-IN",
    "ro-RO",
    "ru-RU",
    "sr-RS",
    "sd-Arab-IN",
    "sk-SK",
    "sl-SI",
    "es-419",
    "sw-KE",
    "sv-SE",
    "tg-TJ",
    "te-IN",
    "th-TH",
    "tr-TR",
    "uk-UA",
    "uz-UZ",
    "vi-VN",
];

pub const XAI_LANGUAGES: [&str; 38] = [
    "ar", "bs", "bg", "yue", "ca", "zh", "hr", "cs", "da", "nl", "en", "fil", "fi", "fr", "de",
    "el", "hi", "hu", "id", "it", "ja", "ko", "mk", "ms", "nb", "fa", "pl", "pt", "ro", "ru", "sk",
    "es", "sv", "th", "tr", "uk", "ur", "vi",
];

pub fn google_languages() -> Vec<&'static str> {
    let mut out: Vec<&str> = Vec::new();
    for code in GOOGLE_LANGUAGES {
        let short = primary(code);
        if !out.contains(&short) {
            out.push(short);
        }
    }
    out
}

fn primary(code: &str) -> &str {
    match code.split('-').next().unwrap_or(code) {
        "cmn" => "zh",
        p => p,
    }
}

pub fn google_code(language: &str) -> String {
    let l = language.to_ascii_lowercase();
    let l = match l.as_str() {
        "no" | "nn" => "nb",
        "tl" => "fil",
        other => other,
    };
    GOOGLE_LANGUAGES
        .iter()
        .find(|c| primary(c) == l)
        .map_or_else(|| language.to_string(), |c| c.to_string())
}

#[derive(Debug, Clone, PartialEq)]
struct Heard {
    start: f64,
    end: f64,
    text: String,
    speaker: Option<u32>,
}

#[derive(Debug, Default)]
struct Got {
    words: Vec<Heard>,
    language: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct Piece {
    file: usize,
    from: i64,
    to: i64,
    at: i64,
}

fn secs(samples: i64) -> f64 {
    samples as f64 / f64::from(RATE)
}

fn padded(stretches: &[(i64, i64)], total: i64) -> Vec<(i64, i64)> {
    let pad = (PAD * f64::from(RATE)) as i64;
    let mut out: Vec<(i64, i64)> = Vec::new();
    for &(s, e) in stretches {
        let (s, e) = ((s - pad).max(0), (e + pad).min(total));
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ if e > s => out.push((s, e)),
            _ => {}
        }
    }
    out
}

fn where_in(pieces: &[Piece], t: f64) -> Option<(usize, f64)> {
    let at = (t * f64::from(RATE)) as i64;
    let i = pieces.partition_point(|p| p.at <= at).saturating_sub(1);
    let p = pieces.get(i)?;
    let into = (at - p.at).clamp(0, p.to - p.from);
    Some((p.file, secs(p.from + into)))
}

pub fn transcribe(plans: &[&RenderPlan], asked: &Asked, options: &Options) -> Result<Vec<Speech>> {
    let company = asked.model.company();
    let key = || {
        options
            .key
            .clone()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| {
                Error::Missing(format!(
                    "no {company} key: set {}, a key command in the preferences, or save one in \
                     the keychain (vidcut services set-key)",
                    key_variable(asked.model)
                ))
            })
    };
    if !asked.send {
        return Err(Error::Invalid(format!(
            "the audio would be sent to {company}: pass send"
        )));
    }
    let started = Instant::now();
    let vad = transcribe::vad_path(asked, options.models)?;
    let channels = plans.first().map_or(1, |p| usize::from(p.channels.max(1)));
    let mut warnings: Vec<Vec<String>> = vec![Vec::new(); plans.len()];
    let mut heard: Vec<Vec<f32>> = Vec::new();
    let mut joined: Vec<f32> = Vec::new();
    let mut pieces: Vec<Piece> = Vec::new();
    let mut found = (asked.find.unwrap_or_default(), None);
    let gap = (GAP * f64::from(RATE)) as usize * channels;
    let upload = upload_folder();
    std::fs::create_dir_all(&upload)?;
    for (i, plan) in plans.iter().enumerate() {
        let pcm = transcribe::heard_plan(plan, &upload, started)?;
        let total = (pcm.len() / channels) as i64;
        let stretches = match channels {
            1 => {
                let f = transcribe::find_speech(&vad, &pcm, asked, &mut warnings[i])?;
                found = (f.found, (f.found == FindSpeech::Sound).then_some(f.floor));
                padded(&f.stretches, total)
            }
            _ => vec![(0, total)],
        };
        for (s, e) in stretches {
            if !joined.is_empty() {
                joined.extend(std::iter::repeat_n(0.0, gap));
            }
            pieces.push(Piece {
                file: i,
                from: s,
                to: e,
                at: (joined.len() / channels) as i64,
            });
            joined.extend_from_slice(&pcm[s as usize * channels..e as usize * channels]);
        }
        heard.push(match channels {
            1 => pcm,
            _ => Vec::new(),
        });
    }
    let length = (joined.len() / channels) as i64;
    let requests: Vec<(i64, i64)> = match asked.model {
        _ if length == 0 => Vec::new(),
        SpeechModel::Google => split(&pieces, length, (GOOGLE_LONGEST * f64::from(RATE)) as i64),
        _ => vec![(0, length)],
    };
    let run = transcribe::run_id();
    let mut words: Vec<Heard> = Vec::new();
    let mut language = None;
    let mut reused = 0;
    for (k, &(a, b)) in requests.iter().enumerate() {
        crate::cancel::stop()?;
        let audio = &joined[a as usize * channels..b as usize * channels];
        let asking = match asked.model {
            SpeechModel::Xai => json!(xai_fields(asked)),
            _ => google_request(asked, ""),
        };
        let reply_key = reply_key(asked.model.company(), &asking, channels, audio);
        let text = match kept_reply(&reply_key) {
            Some(text) => {
                reused += 1;
                text
            }
            None => {
                let key = key()?;
                let path = upload.join(format!("{run}-{k}.flac"));
                let text =
                    write_flac(audio, channels as u16, &path).and_then(|()| match asked.model {
                        SpeechModel::Xai => {
                            let bytes = std::fs::metadata(&path)?.len();
                            if bytes > XAI_LIMIT {
                                return Err(Error::Invalid(format!(
                                    "{} of audio is {:.0} MB of FLAC, over xAI's 500 MB: hear \
                                     less, or the In–Out range",
                                    clock(secs(b - a)),
                                    bytes as f64 / 1e6
                                )));
                            }
                            xai(&key, &path, asked, started)
                        }
                        _ => google(&key, &path, asked, started),
                    });
                let _ = std::fs::remove_file(&path);
                let text = text?;
                keep_reply(&reply_key, &text);
                text
            }
        };
        let got = match asked.model {
            SpeechModel::Xai => xai_words(&text)?,
            _ => google_words(&text)?,
        };
        let shift = secs(a);
        words.extend(got.words.into_iter().map(|w| Heard {
            start: w.start + shift,
            end: w.end + shift,
            ..w
        }));
        language = language.or(got.language);
    }
    let service_speakers = asked.speakers
        && (asked.model == SpeechModel::Xai || requests.len() <= 1)
        && !asked.xai.as_ref().is_some_and(|x| x.multichannel);
    let channel_speakers = asked.speakers && asked.xai.as_ref().is_some_and(|x| x.multichannel);
    let mut per_file: Vec<Vec<Word>> = vec![Vec::new(); plans.len()];
    for w in words {
        let Some((file, start)) = where_in(&pieces, w.start) else {
            continue;
        };
        let end = start + (w.end - w.start).max(0.01);
        per_file[file].push(Word {
            start,
            end,
            text: w.text,
            joined: false,
            speaker: match service_speakers || channel_speakers {
                true => w.speaker,
                false => None,
            },
        });
    }
    if reused > 0 {
        for w in &mut warnings {
            w.push(format!(
                "{company}'s reply kept from before was used: nothing was sent or charged"
            ));
        }
    }
    let longest = requests
        .iter()
        .map(|(a, b)| secs(b - a))
        .fold(0.0, f64::max);
    let mut out = Vec::new();
    for (i, mut said_words) in per_file.into_iter().enumerate() {
        said_words.sort_by(|x, y| x.start.total_cmp(&y.start));
        let mut said: Vec<Said> = match (said_words.first(), said_words.last()) {
            (Some(f), Some(l)) => vec![Said {
                start: f.start,
                end: l.end,
                text: String::new(),
                words: said_words.clone(),
                speech: Vec::new(),
            }],
            _ => Vec::new(),
        };
        let service = match asked.model {
            SpeechModel::Google => "google",
            _ => "xai",
        };
        let speakers = match asked.speakers {
            false => None,
            true if service_speakers || channel_speakers => Some((run.clone(), service)),
            true if !heard[i].is_empty() => {
                let (turns, by) = transcribe::speakers_of(
                    options.models,
                    &heard[i],
                    options.gpu,
                    language.as_deref().or(asked.language.as_deref()),
                    asked.speaker_model,
                    started,
                    &mut warnings[i],
                )?;
                transcript::give_speakers(&mut said, &turns);
                Some((transcribe::run_id(), by))
            }
            true => None,
        };
        out.push(Speech {
            model: asked.model_name(),
            device: format!("{company}'s servers"),
            language: language.clone().or_else(|| asked.language.clone()),
            timing: Timing::Words,
            pieces: requests.len(),
            longest_piece: longest,
            said,
            warnings: std::mem::take(&mut warnings[i]),
            found: found.0,
            sound_db: found.1,
            speakers_by: speakers.as_ref().map(|(_, by)| by.to_string()),
            speakers: speakers.map(|(run, _)| run),
        });
    }
    Ok(out)
}

fn split(pieces: &[Piece], length: i64, max: i64) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    let mut start = 0;
    while length - start > max {
        let cut = pieces
            .iter()
            .map(|p| p.at)
            .filter(|&at| at > start && at - start <= max)
            .max()
            .unwrap_or(start + max);
        out.push((start, cut));
        start = cut;
    }
    out.push((start, length));
    out
}

fn key_variable(model: SpeechModel) -> &'static str {
    match model {
        SpeechModel::Google => "GEMINI_API_KEY",
        _ => "XAI_API_KEY",
    }
}

fn clock(secs: f64) -> String {
    vidcut_core::subtitles::clock(secs, '.')
}

fn write_flac(samples: &[f32], channels: u16, path: &Path) -> Result<()> {
    crate::init();
    let codec = encoder::find(codec::Id::FLAC)
        .ok_or_else(|| Error::Missing("this FFmpeg has no FLAC encoder".into()))?;
    let mut octx = format::output_as(path, "flac").ctx(|| format!("writing {}", path.display()))?;
    let layout = ffmpeg::ChannelLayout::default(i32::from(channels));
    let sample = format::Sample::I16(format::sample::Type::Packed);
    let mut ctx = codec::context::Context::new_with_codec(codec)
        .encoder()
        .audio()
        .ctx(|| "creating the FLAC encoder".into())?;
    ctx.set_rate(RATE as i32);
    ctx.set_channel_layout(layout);
    ctx.set_format(sample);
    ctx.set_time_base(ffmpeg::Rational(1, RATE as i32));
    let mut enc = ctx
        .open_as(codec)
        .ctx(|| "opening the FLAC encoder".into())?;
    let mut ost = octx.add_stream(codec).ctx(|| "adding the sound".into())?;
    ost.set_parameters(&enc);
    ost.set_time_base(ffmpeg::Rational(1, RATE as i32));
    octx.write_header()
        .ctx(|| format!("writing {}", path.display()))?;
    let tb = octx
        .stream(0)
        .map_or(ffmpeg::Rational(1, RATE as i32), |s| s.time_base());
    let ch = usize::from(channels);
    let size = match enc.frame_size() {
        0 => 4608,
        n => n as usize,
    };
    let drain = |enc: &mut encoder::Audio, octx: &mut format::context::Output| -> Result<()> {
        let mut packet = ffmpeg::Packet::empty();
        while enc.receive_packet(&mut packet).is_ok() {
            packet.set_stream(0);
            packet.rescale_ts(ffmpeg::Rational(1, RATE as i32), tb);
            packet
                .write_interleaved(octx)
                .ctx(|| "writing FLAC".into())?;
        }
        Ok(())
    };
    for (k, chunk) in samples.chunks(size * ch).enumerate() {
        crate::cancel::stop()?;
        let n = chunk.len() / ch;
        let mut f = frame::Audio::new(sample, n, layout);
        f.set_rate(RATE);
        let data = f.data_mut(0);
        for (i, s) in chunk.iter().enumerate() {
            let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            data[i * 2..i * 2 + 2].copy_from_slice(&v.to_le_bytes());
        }
        f.set_pts(Some((k * size) as i64));
        enc.send_frame(&f).ctx(|| "encoding FLAC".into())?;
        drain(&mut enc, &mut octx)?;
    }
    enc.send_eof().ctx(|| "encoding FLAC".into())?;
    drain(&mut enc, &mut octx)?;
    octx.write_trailer()
        .ctx(|| format!("writing {}", path.display()))?;
    Ok(())
}

struct Counted<R> {
    inner: R,
    sent: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.stop.load(Ordering::Relaxed) {
            return Err(std::io::Error::other(CANCELLED));
        }
        let n = self.inner.read(buf)?;
        self.sent.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_recv_response(Some(WAIT))
        .build()
        .into()
}

struct Answer {
    status: u16,
    text: String,
    upload_url: Option<String>,
}

fn answer(r: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Answer> {
    let r = r.map_err(|e| Error::Invalid(e.to_string()))?;
    let status = r.status().as_u16();
    let upload_url = r
        .headers()
        .get("x-goog-upload-url")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let text = r
        .into_body()
        .with_config()
        .limit(256 << 20)
        .read_to_string()
        .map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(Answer {
        status,
        text,
        upload_url,
    })
}

fn refused(company: &str, a: &Answer) -> Error {
    let v: Value = serde_json::from_str(&a.text).unwrap_or(Value::Null);
    let message = [
        &v["error"]["message"],
        &v["error"],
        &v["message"],
        &v["detail"],
    ]
    .into_iter()
    .find_map(|m| m.as_str().map(String::from))
    .unwrap_or_else(|| a.text.chars().take(300).collect());
    Error::Invalid(format!("{company}: HTTP {}: {message}", a.status))
}

fn upload_in_thread<F>(path: &Path, started: Instant, send: F) -> Result<Answer>
where
    F: FnOnce(Counted<std::fs::File>, u64) -> Result<Answer> + Send + 'static,
{
    let file = std::fs::File::open(path)?;
    let total = file.metadata()?.len();
    let sent = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let body = Counted {
        inner: file,
        sent: sent.clone(),
        stop: stop.clone(),
    };
    waiting(path, started, total, sent, stop, move || send(body, total))
}

fn waiting<F>(
    path: &Path,
    started: Instant,
    total: u64,
    sent: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    send: F,
) -> Result<Answer>
where
    F: FnOnce() -> Result<Answer> + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("vidcut-upload".into())
        .spawn(move || {
            let _ = tx.send(send());
        })?;
    loop {
        if crate::cancel::cancelled() {
            stop.store(true, Ordering::Relaxed);
            return Err(Error::Invalid(CANCELLED.into()));
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(r) => return r,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(Error::Invalid("the upload stopped".into()));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        let done = sent.load(Ordering::Relaxed);
        let stage = match done < total {
            true => Stage::Uploading,
            false => Stage::Waiting,
        };
        crate::progress::report(
            path,
            Progress::stage(stage, done as i64, total as i64, started.elapsed()),
        );
    }
}

fn xai_fields(asked: &Asked) -> Vec<(&'static str, String)> {
    let o = asked.xai.clone().unwrap_or_default();
    let mut fields = vec![("model", o.model.clone().unwrap_or_else(|| XAI_MODEL.into()))];
    if let Some(l) = &asked.language {
        fields.push(("language", l.clone()));
    }
    if o.format {
        fields.push(("format", "true".into()));
    }
    if o.multichannel {
        fields.push(("multichannel", "true".into()));
    } else if asked.speakers {
        fields.push(("diarize", "true".into()));
    }
    for t in &o.keyterm {
        fields.push(("keyterm", t.clone()));
    }
    let fillers = o.filler_words.unwrap_or(true);
    fields.push(("filler_words", fillers.to_string()));
    let gate = o
        .vad_threshold
        .or((asked.find == Some(FindSpeech::Sound)).then_some(0.0));
    if let Some(v) = gate {
        fields.push(("vad_threshold", v.to_string()));
    }
    fields
}

fn xai(key: &str, path: &Path, asked: &Asked, started: Instant) -> Result<String> {
    let boundary = format!("vidcut-{}", transcribe::run_id());
    let mut head = String::new();
    for (name, value) in xai_fields(asked) {
        head.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ));
    }
    head.push_str(&format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"speech.flac\"\r\n\
         Content-Type: audio/flac\r\n\r\n"
    ));
    let tail = format!("\r\n--{boundary}--\r\n");
    let auth = format!("Bearer {key}");
    let a = upload_in_thread(path, started, move |file, bytes| {
        let length = head.len() as u64 + bytes + tail.len() as u64;
        let body = std::io::Cursor::new(head.into_bytes())
            .chain(file)
            .chain(std::io::Cursor::new(tail.into_bytes()));
        answer(
            agent()
                .post(XAI_URL)
                .header("Authorization", &auth)
                .header(
                    "Content-Type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .header("Content-Length", length.to_string())
                .send(ureq::SendBody::from_owned_reader(body)),
        )
    })?;
    if a.status != 200 {
        return Err(refused("xAI", &a));
    }
    xai_words(&a.text)?;
    Ok(a.text)
}

fn xai_words(text: &str) -> Result<Got> {
    let v: Value = serde_json::from_str(text)
        .map_err(|e| Error::Invalid(format!("xAI's answer isn't JSON: {e}")))?;
    let word = |w: &Value, channel: Option<u32>| -> Option<Heard> {
        Some(Heard {
            start: w["start"].as_f64()?,
            end: w["end"].as_f64()?,
            text: w["text"].as_str()?.trim().to_string(),
            speaker: channel.or(w["speaker"].as_u64().map(|s| s as u32 + 1)),
        })
    };
    let mut words: Vec<Heard> = match v["channels"].as_array() {
        Some(channels) => channels
            .iter()
            .flat_map(|c| {
                let index = c["index"].as_u64().map(|i| i as u32 + 1);
                c["words"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(move |w| word(w, index))
                    .collect::<Vec<_>>()
            })
            .collect(),
        None => v["words"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|w| word(w, None))
            .collect(),
    };
    words.retain(|w| !w.text.is_empty());
    words.sort_by(|a, b| a.start.total_cmp(&b.start));
    Ok(Got {
        words,
        language: v["language"]
            .as_str()
            .map(|l| primary(l).to_ascii_lowercase()),
    })
}

fn google(key: &str, path: &Path, asked: &Asked, started: Instant) -> Result<String> {
    let bytes = std::fs::metadata(path)?.len();
    let start = answer(
        agent()
            .post(&format!("{GOOGLE}/upload/v1beta/files"))
            .header("x-goog-api-key", key)
            .header("X-Goog-Upload-Protocol", "resumable")
            .header("X-Goog-Upload-Command", "start")
            .header("X-Goog-Upload-Header-Content-Length", bytes.to_string())
            .header("X-Goog-Upload-Header-Content-Type", "audio/flac")
            .header("Content-Type", "application/json")
            .send(json!({ "file": { "display_name": "vidcut speech" } }).to_string()),
    )?;
    if start.status != 200 {
        return Err(refused("Google", &start));
    }
    let url = start
        .upload_url
        .ok_or_else(|| Error::Invalid("Google gave no upload address".into()))?;
    let uploaded = upload_in_thread(path, started, move |file, bytes| {
        answer(
            agent()
                .post(&url)
                .header("Content-Length", bytes.to_string())
                .header("X-Goog-Upload-Offset", "0")
                .header("X-Goog-Upload-Command", "upload, finalize")
                .send(ureq::SendBody::from_owned_reader(file)),
        )
    })?;
    if uploaded.status != 200 {
        return Err(refused("Google", &uploaded));
    }
    let file: Value = serde_json::from_str(&uploaded.text).unwrap_or(Value::Null);
    let name = file["file"]["name"]
        .as_str()
        .ok_or_else(|| Error::Invalid("Google's upload answer has no file name".into()))?
        .to_string();
    let uri = file["file"]["uri"].as_str().unwrap_or_default().to_string();
    let got = google_transcribe(key, &name, &uri, asked, started);
    let _ = agent()
        .delete(&format!("{GOOGLE}/v1beta/{name}"))
        .header("x-goog-api-key", key)
        .call();
    got
}

fn google_transcribe(
    key: &str,
    name: &str,
    uri: &str,
    asked: &Asked,
    started: Instant,
) -> Result<String> {
    for _ in 0..300 {
        crate::cancel::stop()?;
        let a = answer(
            agent()
                .get(&format!("{GOOGLE}/v1beta/{name}"))
                .header("x-goog-api-key", key)
                .call(),
        )?;
        if a.status != 200 {
            return Err(refused("Google", &a));
        }
        let v: Value = serde_json::from_str(&a.text).unwrap_or(Value::Null);
        match v["state"].as_str() {
            Some("ACTIVE") | None => break,
            Some("FAILED") => {
                return Err(Error::Invalid(format!(
                    "Google couldn't read the audio: {}",
                    v["error"]["message"].as_str().unwrap_or("no reason given")
                )));
            }
            _ => std::thread::sleep(Duration::from_secs(2)),
        }
    }
    let body = google_request(asked, uri).to_string();
    let key = key.to_string();
    let a = waiting(
        &upload_folder(),
        started,
        0,
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicBool::new(false)),
        move || {
            answer(
                agent()
                    .post(&format!("{GOOGLE}/v1beta/interactions"))
                    .header("x-goog-api-key", &key)
                    .header("Content-Type", "application/json")
                    .send(body),
            )
        },
    )?;
    if a.status != 200 {
        return Err(refused("Google", &a));
    }
    google_words(&a.text)?;
    Ok(a.text)
}

fn google_request(asked: &Asked, uri: &str) -> Value {
    let o = asked.google.clone().unwrap_or_default();
    let codes: Vec<String> = match (&o.language_codes[..], &asked.language) {
        ([], Some(l)) => vec![google_code(l)],
        (codes, _) => codes.to_vec(),
    };
    let mut mode = json!({ "type": "verbatim", "timestamp_granularities": ["word"] });
    if asked.speakers {
        mode["diarization_mode"] = json!("speaker");
    }
    let mut config = json!({ "mode": mode });
    if !codes.is_empty() {
        config["language_codes"] = json!(codes);
    }
    json!({
        "model": o.model.unwrap_or_else(|| GOOGLE_MODEL.into()),
        "input": [{ "type": "audio", "uri": uri, "mime_type": "audio/flac" }],
        "generation_config": { "transcription_config": config },
    })
}

fn offset(v: &Value) -> Option<f64> {
    v.as_str()?.trim_end_matches('s').parse().ok()
}

fn google_words(text: &str) -> Result<Got> {
    let v: Value = serde_json::from_str(text)
        .map_err(|e| Error::Invalid(format!("Google's answer isn't JSON: {e}")))?;
    let mut words = Vec::new();
    for step in v["steps"].as_array().into_iter().flatten() {
        if step["type"].as_str().is_some_and(|t| t != "model_output") {
            continue;
        }
        for content in step["content"].as_array().into_iter().flatten() {
            for a in content["annotations"].as_array().into_iter().flatten() {
                if a["type"] != "word_info" {
                    continue;
                }
                let (Some(start), Some(end), Some(text)) = (
                    offset(&a["start_offset"]),
                    offset(&a["end_offset"]),
                    a["text"].as_str(),
                ) else {
                    continue;
                };
                let speaker = a["speaker"]
                    .as_str()
                    .and_then(|s| s.trim_start_matches("spk_").parse::<u32>().ok());
                words.push(Heard {
                    start,
                    end,
                    text: text.trim().to_string(),
                    speaker,
                });
            }
        }
    }
    words.retain(|w| !w.text.is_empty());
    Ok(Got {
        words,
        language: None,
    })
}

pub fn upload_folder() -> PathBuf {
    vidcut_core::dirs::cache().join("uploads")
}

pub fn replies_folder() -> PathBuf {
    vidcut_core::dirs::cache().join("speech-replies")
}

fn sorted(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for k in keys {
                out.insert(k.clone(), sorted(&m[k]));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
        v => v.clone(),
    }
}

fn reply_key(service: &str, asking: &Value, channels: usize, audio: &[f32]) -> String {
    let mut h = Sha256::new();
    h.update(service.as_bytes());
    h.update(sorted(asking).to_string().as_bytes());
    h.update(channels.to_le_bytes());
    for s in audio {
        h.update(((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn kept_reply(key: &str) -> Option<String> {
    std::fs::read_to_string(replies_folder().join(format!("{key}.json"))).ok()
}

fn keep_reply(key: &str, text: &str) {
    let dir = replies_folder();
    let part = dir.join(format!("{key}.part"));
    let _ = std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(&part, text))
        .and_then(|()| std::fs::rename(&part, dir.join(format!("{key}.json"))));
}

#[cfg(test)]
mod tests {
    use super::{Heard, google_code, google_words, offset, reply_key, xai_words};
    use serde_json::json;

    #[test]
    fn the_order_of_the_settings_doesnt_change_the_reply_key() {
        let a = reply_key("xAI", &json!({"a": 1, "b": 2}), 1, &[0.5]);
        let b = reply_key("xAI", &json!({"b": 2, "a": 1}), 1, &[0.5]);
        assert_eq!(a, b);
    }

    #[test]
    fn other_audio_has_another_reply_key() {
        let a = reply_key("xAI", &json!({}), 1, &[0.5]);
        let b = reply_key("xAI", &json!({}), 1, &[0.25]);
        assert_ne!(a, b);
    }

    #[test]
    fn a_two_letter_language_takes_googles_regional_code() {
        assert_eq!(google_code("fi"), "fi-FI");
    }

    #[test]
    fn chinese_is_googles_mandarin() {
        assert_eq!(google_code("zh"), "cmn-Hans-CN");
    }

    #[test]
    fn a_google_offset_reads_its_seconds() {
        assert_eq!(offset(&json!("0.450s")), Some(0.45));
    }

    #[test]
    fn xai_speakers_count_from_one() {
        let got = xai_words(
            r#"{"text":"hi","language":"en","duration":1.0,
                "words":[{"text":"hi","start":0.1,"end":0.4,"speaker":0}]}"#,
        )
        .unwrap();
        assert_eq!(
            got.words,
            [Heard {
                start: 0.1,
                end: 0.4,
                text: "hi".into(),
                speaker: Some(1)
            }]
        );
    }

    #[test]
    fn an_xai_channel_is_its_words_speaker() {
        let got = xai_words(
            r#"{"text":"a b","language":"en","duration":1.0,"channels":[
                {"index":1,"text":"b","words":[{"text":"b","start":0.5,"end":0.7}]}]}"#,
        )
        .unwrap();
        assert_eq!(got.words[0].speaker, Some(2));
    }

    #[test]
    fn google_words_come_from_the_word_annotations() {
        let got = google_words(
            r#"{"steps":[{"type":"model_output","content":[{"type":"text","text":"Hello",
                "annotations":[{"type":"word_info","text":"Hello","speaker":"spk_2",
                "start_offset":"0.100s","end_offset":"0.450s"}]}]}]}"#,
        )
        .unwrap();
        assert_eq!(
            got.words,
            [Heard {
                start: 0.1,
                end: 0.45,
                text: "Hello".into(),
                speaker: Some(2)
            }]
        );
    }
}
