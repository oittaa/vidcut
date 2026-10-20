use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::request::{FindSpeech, GoogleOptions, SpeakerModel, SpeechModel, XaiOptions};
use crate::time::Rational;

pub const RATE: u32 = 16_000;
const PAUSE: f64 = 1.0;
const SHORTEST_CUE: f64 = 5.0 / 6.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Asked {
    pub model: SpeechModel,
    pub model_file: Option<PathBuf>,
    pub vad_file: Option<PathBuf>,
    pub language: Option<String>,
    pub download: bool,
    pub find: Option<FindSpeech>,
    pub sound_db: Option<f64>,
    pub speakers: bool,
    pub speaker_model: SpeakerModel,
    pub send: bool,
    pub xai: Option<XaiOptions>,
    pub google: Option<GoogleOptions>,
}

impl Asked {
    pub fn model_name(&self) -> String {
        match &self.model_file {
            Some(f) => f.file_name().map_or_else(
                || f.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            ),
            None => match (self.model, &self.xai, &self.google) {
                (SpeechModel::Xai, Some(XaiOptions { model: Some(m), .. }), _)
                | (SpeechModel::Google, _, Some(GoogleOptions { model: Some(m), .. })) => m.clone(),
                (m, _, _) => m.title().to_string(),
            },
        }
    }

    pub fn level(&self) -> Option<f64> {
        (self.find == Some(FindSpeech::Sound)).then(|| self.sound_db.unwrap_or(SOUND_DB))
    }

    pub fn made(&self, speech: &Speech) -> bool {
        speech.model == self.model_name()
            && self
                .language
                .as_ref()
                .is_none_or(|l| speech.language.as_ref() == Some(l))
            && self.find.is_none_or(|f| {
                speech.found == f && (f == FindSpeech::Voice || speech.sound_db == self.level())
            })
            && (!self.speakers || speech.speakers.is_some())
            && (!self.speakers
                || self.speaker_model == SpeakerModel::Auto
                || speech.speakers_by.as_deref() == Some(self.speaker_model.name()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Timing {
    Words,
    Phrases,
    Pauses,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Speech {
    pub model: String,
    pub device: String,
    pub language: Option<String>,
    pub timing: Timing,
    pub pieces: usize,
    pub longest_piece: f64,
    pub said: Vec<Said>,
    pub warnings: Vec<String>,
    #[serde(default)]
    pub found: FindSpeech,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound_db: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speakers: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speakers_by: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Said {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub words: Vec<Word>,
    pub speech: Vec<(f64, f64)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub joined: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Turn {
    pub start: f64,
    pub end: f64,
    pub speaker: u32,
}

pub const NEAREST_TURN: f64 = 1.0;

pub fn give_speakers(said: &mut [Said], turns: &[Turn]) {
    for w in said.iter_mut().flat_map(|s| s.words.iter_mut()) {
        let overlap = |t: &Turn| w.end.min(t.end) - w.start.max(t.start);
        w.speaker = turns
            .iter()
            .filter(|t| overlap(t) > -NEAREST_TURN)
            .max_by(|a, b| overlap(a).total_cmp(&overlap(b)))
            .map(|t| t.speaker);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub chars: usize,
    pub lines: usize,
    pub gap: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Made {
    pub start: i64,
    pub end: i64,
    pub text: String,
    pub words: Vec<i64>,
    pub speaker: Option<u32>,
}

pub const FLAC_MB_PER_MINUTE: f64 = 1.0;

pub fn upload_mb(secs: f64, channels: u16) -> f64 {
    secs / 60.0 * FLAC_MB_PER_MINUTE * f64::from(channels.max(1))
}

pub fn cost(model: SpeechModel, secs: f64) -> f64 {
    model.dollars_per_hour() * secs / 3600.0
}

pub fn dollars(v: f64) -> String {
    match v {
        v if v < 0.005 => "less than $0.01".into(),
        v => format!("about ${v:.2}"),
    }
}

pub const SOUND_DB: f64 = -40.0;
pub const SHORTEST_QUIET: f64 = 0.5;
pub const SHORTEST_SOUND: f64 = 0.25;

pub fn sounding(levels: &[f32], frame: usize, floor_db: f64) -> Vec<(i64, i64)> {
    let frame = frame.max(1);
    let per_sec = f64::from(RATE) / frame as f64;
    let quiet_frames = (SHORTEST_QUIET * per_sec).ceil() as usize;
    let sound_frames = (SHORTEST_SOUND * per_sec).ceil() as usize;
    let floor = 10f32.powf(floor_db as f32 / 20.0);
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for (i, &l) in levels.iter().enumerate() {
        if l < floor {
            continue;
        }
        match runs.last_mut() {
            Some((_, end)) if i - *end < quiet_frames => *end = i + 1,
            _ => runs.push((i, i + 1)),
        }
    }
    runs.into_iter()
        .filter(|(a, b)| b - a >= sound_frames)
        .map(|(a, b)| ((a * frame) as i64, (b * frame) as i64))
        .collect()
}

pub fn pieces(
    speech: &[(i64, i64)],
    quiet: &[f32],
    frame: usize,
    max: i64,
    pad: i64,
) -> Vec<(i64, i64)> {
    let frame = frame.max(1) as i64;
    let limit = (max - 2 * pad).max(frame);
    let mut stretches = Vec::new();
    for &(s, e) in speech.iter().filter(|(s, e)| e > s) {
        let mut s = s;
        while e - s > limit {
            let cut = quietest(quiet, frame, s + limit / 2, s + limit);
            stretches.push((s, cut, true));
            s = cut;
        }
        stretches.push((s, e, false));
    }
    let n = stretches.len();
    let head = |i: usize| -> i64 {
        let (s, _, _) = stretches[i];
        match i.checked_sub(1).map(|j| stretches[j]) {
            Some((_, pe, true)) if pe == s => s,
            Some((_, pe, _)) => (s - pad).max((pe + s) / 2),
            None => (s - pad).max(0),
        }
    };
    let tail = |i: usize| -> i64 {
        let (_, e, cut) = stretches[i];
        match stretches.get(i + 1) {
            _ if cut => e,
            Some(&(ns, _, _)) => (e + pad).min((e + ns) / 2),
            None => e + pad,
        }
    };
    let mut out = Vec::new();
    let mut first = 0;
    while first < n {
        let start = head(first);
        let mut last = first;
        while last + 1 < n && tail(last + 1) - start <= max {
            last += 1;
        }
        if last + 1 < n {
            let pause = |i: usize| match stretches[i] {
                (_, _, true) => -1,
                (_, e, _) => stretches[i + 1].0 - e,
            };
            let late = (first..=last).filter(|&i| tail(i) - start >= max / 2);
            if let Some(best) = late.max_by_key(|&i| (pause(i), i)) {
                last = best;
            }
        }
        out.push((start, tail(last)));
        first = last + 1;
    }
    out
}

fn quietest(quiet: &[f32], frame: i64, from: i64, to: i64) -> i64 {
    let (a, b) = (from / frame, (to / frame).max(from / frame + 1));
    (a..b)
        .min_by(|&x, &y| {
            let level = |k: i64| quiet.get(k as usize).copied().unwrap_or(f32::MAX);
            level(x).total_cmp(&level(y)).then(y.cmp(&x))
        })
        .map_or(to, |k| (k * frame + frame / 2).clamp(from, to))
}

pub fn spread(text: &str, speech: &[(f64, f64)]) -> Vec<Word> {
    let tokens: Vec<(String, bool)> = text
        .split_whitespace()
        .flat_map(|w| {
            let chars: Vec<char> = w.chars().collect();
            let wide = chars.iter().any(|&c| is_wide(c));
            let size = if wide { 1 } else { chars.len().max(1) };
            chars
                .chunks(size)
                .enumerate()
                .map(|(i, c)| (c.iter().collect::<String>(), i > 0))
                .collect::<Vec<_>>()
        })
        .collect();
    let total: f64 = speech.iter().map(|(a, b)| (b - a).max(0.0)).sum();
    let weight = |t: &(String, bool)| t.0.chars().count() as f64 + if t.1 { 0.0 } else { 1.0 };
    let all: f64 = tokens.iter().map(weight).sum();
    if tokens.is_empty() || total <= 0.0 || all <= 0.0 {
        return Vec::new();
    }
    let at = |share: f64| -> f64 {
        let mut left = share.clamp(0.0, 1.0) * total;
        for &(a, b) in speech {
            let len = (b - a).max(0.0);
            if left <= len {
                return a + left;
            }
            left -= len;
        }
        speech.last().map_or(0.0, |s| s.1)
    };
    let mut done = 0.0;
    tokens
        .into_iter()
        .map(|t| {
            let w = weight(&t);
            let start = at((done + if t.1 { 0.0 } else { 1.0 }) / all);
            done += w;
            Word {
                start,
                end: at(done / all),
                text: t.0,
                joined: t.1,
                speaker: None,
            }
        })
        .collect()
}

fn is_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF | 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x3FFFF)
}

pub const FILLER_LANGUAGES: [&str; 25] = [
    "en", "de", "fr", "es", "it", "pt", "ru", "uk", "hr", "sl", "lv", "lt", "et", "fi", "sv", "da",
    "nl", "pl", "cs", "sk", "hu", "ro", "bg", "el", "mt",
];

pub fn fillers(language: &str) -> &'static [&'static str] {
    match language {
        "en" => &[
            "um", "umm", "uh", "uhh", "uhm", "er", "erm", "hmm", "hm", "mmm", "mhm",
        ],
        "de" => &["äh", "ähm", "öh", "öhm", "hm", "hmm", "mhm"],
        "fr" => &["euh", "heu", "hum", "hmm"],
        "es" => &["eh", "em", "ehm", "emm", "mmm", "hmm"],
        "it" => &["eh", "ehm", "ehh", "mmm", "hmm"],
        "pt" => &["hum", "hmm", "ahn", "hã", "éé"],
        "ru" => &["э", "ээ", "эээ", "эм", "ммм", "хм"],
        "uk" => &["ее", "еее", "ем", "ммм", "хм"],
        "hr" | "sl" | "cs" | "sk" => &["eee", "ee", "ehm", "hmm", "mmm"],
        "lv" => &["ēē", "ēēē", "hmm", "mmm"],
        "lt" => &["ėė", "ėėė", "hmm", "mmm"],
        "et" => &["ee", "eee", "õõ", "õõõ", "hmm"],
        "fi" => &["öö", "ööh", "ää", "äää", "hmm", "mmm"],
        "sv" => &["öh", "öhm", "eh", "ehm", "hmm", "mmm"],
        "da" => &["øh", "øhm", "øøh", "hmm", "mmm"],
        "nl" => &["eh", "ehm", "uh", "uhm", "eeh", "hmm"],
        "pl" => &["yyy", "yy", "eee", "ee", "hmm", "mmm"],
        "hu" => &["öö", "ööö", "hmm", "mmm"],
        "ro" => &["ăă", "ăăă", "îî", "hmm", "mmm"],
        "bg" => &["ъъ", "ъъъ", "ъм", "ммм", "хм"],
        "el" => &["εε", "εεε", "εμ", "χμ"],
        "mt" => &["ehm", "mmm", "hmm"],
        _ => &[],
    }
}

const COMMON: [(&str, [&str; 6]); 25] = [
    ("en", ["the", "and", "of", "to", "is", "that"]),
    ("de", ["der", "die", "und", "das", "ist", "nicht"]),
    ("fr", ["le", "les", "et", "est", "des", "une"]),
    ("es", ["el", "los", "que", "y", "una", "del"]),
    ("it", ["il", "che", "di", "non", "gli", "una"]),
    ("pt", ["o", "os", "não", "uma", "do", "com"]),
    ("ru", ["и", "в", "не", "что", "на", "это"]),
    ("uk", ["і", "в", "не", "що", "на", "це"]),
    ("hr", ["je", "i", "da", "se", "u", "na"]),
    ("sl", ["je", "in", "da", "se", "v", "ki"]),
    ("lv", ["un", "ir", "ka", "ar", "uz", "no"]),
    ("lt", ["ir", "kad", "tai", "su", "yra", "ne"]),
    ("et", ["ja", "on", "et", "ei", "see", "ka"]),
    ("fi", ["ja", "on", "että", "ei", "se", "oli"]),
    ("sv", ["och", "att", "det", "är", "som", "inte"]),
    ("da", ["og", "at", "det", "er", "som", "ikke"]),
    ("nl", ["de", "het", "een", "en", "van", "niet"]),
    ("pl", ["i", "w", "nie", "się", "na", "że"]),
    ("cs", ["a", "je", "se", "na", "že", "to"]),
    ("sk", ["a", "je", "sa", "na", "že", "to"]),
    ("hu", ["a", "az", "és", "hogy", "nem", "egy"]),
    ("ro", ["și", "de", "în", "nu", "că", "este"]),
    ("bg", ["и", "на", "да", "се", "не", "е"]),
    ("el", ["και", "το", "να", "η", "του", "είναι"]),
    ("mt", ["il", "u", "li", "ta", "fil", "huwa"]),
];

fn bare(text: &str) -> String {
    text.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

pub fn language_of<'a>(texts: impl Iterator<Item = &'a str>) -> Option<&'static str> {
    let mut score = [0usize; COMMON.len()];
    for text in texts {
        let word = bare(text);
        for (k, (_, common)) in COMMON.iter().enumerate() {
            if common.contains(&word.as_str()) {
                score[k] += 1;
            }
        }
    }
    let (k, best) = score.iter().enumerate().max_by_key(|(_, s)| **s)?;
    (*best > 0).then_some(COMMON[k].0)
}

pub fn built_in_fillers(language: &str) -> Vec<String> {
    fillers(language).iter().map(|w| w.to_string()).collect()
}

pub fn same_word(a: &str, b: &str) -> bool {
    bare(a) == bare(b)
}

pub fn is_filler(text: &str, fillers: &[String]) -> bool {
    let word = bare(text);
    fillers.contains(&word)
}

pub fn filler_list(words: &[String]) -> std::result::Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for w in words {
        if w.trim().chars().any(char::is_whitespace) {
            return Err(format!("a filler word is one word: {:?}", w.trim()));
        }
        let w = bare(w);
        if !w.is_empty() && !out.contains(&w) {
            out.push(w);
        }
    }
    Ok(out)
}

pub fn spoken(speech: &Speech) -> std::result::Result<Vec<Word>, String> {
    match speech.timing {
        Timing::Words => Ok(speech
            .said
            .iter()
            .flat_map(|s| s.words.iter().cloned())
            .filter(|w| !w.text.trim().is_empty() && w.end > w.start)
            .collect()),
        Timing::Phrases => Err(format!("{} times phrases, not words", speech.model)),
        Timing::Pauses => Err(format!("{} has no word times", speech.model)),
    }
}

pub fn word_frames(
    words: &[Word],
    fps: Rational,
    origin: i64,
    end: i64,
) -> Vec<(String, i64, i64)> {
    let mut out = Vec::new();
    for w in words {
        let start = origin + fps.secs_to_frames(w.start);
        if start >= end {
            continue;
        }
        let start = start.max(origin).max(0);
        let stop = (origin + fps.secs_to_frames(w.end)).clamp(start + 1, end.max(start + 1));
        if stop <= start {
            continue;
        }
        out.push((w.text.clone(), start, stop));
    }
    out
}

pub fn words(said: &Said) -> Vec<Word> {
    if !said.words.is_empty() {
        return said.words.clone();
    }
    if said.speech.is_empty() {
        spread(&said.text, &[(said.start, said.end)])
    } else {
        spread(&said.text, &said.speech)
    }
}

fn ends_sentence(w: &str) -> bool {
    w.trim_end_matches(['"', '\'', ')', '”', '’', '»'])
        .ends_with(['.', '?', '!', '…', '。', '？', '！'])
}

fn wrap(words: &[Word], chars: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for w in words {
        let fits = |line: &String| {
            line.chars().count() + w.text.chars().count() + usize::from(!w.joined) <= chars
        };
        match lines.last_mut() {
            Some(line) if fits(line) => {
                if !line.is_empty() && !w.joined {
                    line.push(' ');
                }
                line.push_str(&w.text);
            }
            _ => lines.push(w.text.clone()),
        }
    }
    lines
}

fn sentences(words: Vec<Word>) -> Vec<Vec<Word>> {
    let mut out = Vec::new();
    let mut now: Vec<Word> = Vec::new();
    for w in words.into_iter().filter(|w| !w.text.trim().is_empty()) {
        if now
            .last()
            .is_some_and(|l| w.start - l.end >= PAUSE || l.speaker != w.speaker)
        {
            out.push(std::mem::take(&mut now));
        }
        let ends = ends_sentence(&w.text);
        now.push(w);
        if ends {
            out.push(std::mem::take(&mut now));
        }
    }
    if !now.is_empty() {
        out.push(now);
    }
    out
}

fn ends_clause(w: &str) -> bool {
    w.ends_with([',', ';', ':', '—', '–', '、', '，'])
}

fn split(sentence: Vec<Word>, chars: usize, lines: usize) -> Vec<Vec<Word>> {
    let fits = |words: &[Word]| words.len() == 1 || wrap(words, chars).len() <= lines;
    if fits(&sentence) {
        return vec![sentence];
    }
    let n = sentence.len();
    let mut at = vec![0.0; n + 1];
    for (i, w) in sentence.iter().enumerate() {
        let space = if i > 0 && !w.joined { 1.0 } else { 0.0 };
        at[i + 1] = at[i] + w.text.chars().count() as f64 + space;
    }
    let pull = chars as f64 / 4.0;
    for parts in 2..=n {
        let mut cuts = vec![0];
        for k in 1..parts {
            let goal = at[n] * k as f64 / parts as f64;
            let score = |j: usize| {
                (at[j] - goal).abs()
                    - if ends_clause(&sentence[j - 1].text) {
                        pull
                    } else {
                        0.0
                    }
            };
            let (lo, hi) = (cuts[k - 1] + 1, n - (parts - k));
            let Some(best) = (lo..=hi).min_by(|&x, &y| score(x).total_cmp(&score(y))) else {
                break;
            };
            cuts.push(best);
        }
        cuts.push(n);
        let pieces: Vec<&[Word]> = cuts.windows(2).map(|c| &sentence[c[0]..c[1]]).collect();
        if cuts.len() == parts + 1 && pieces.iter().all(|p| fits(p)) {
            return pieces.into_iter().map(<[Word]>::to_vec).collect();
        }
    }
    sentence.into_iter().map(|w| vec![w]).collect()
}

pub fn speaker_count(said: &[Said]) -> u32 {
    said.iter()
        .flat_map(|s| s.words.iter())
        .filter_map(|w| w.speaker)
        .max()
        .unwrap_or(0)
}

pub fn cues(said: &[Said], fps: Rational, layout: Layout) -> Vec<Made> {
    let chars = layout.chars.max(1);
    let lines = layout.lines.max(1);
    let groups: Vec<Vec<Word>> = said
        .iter()
        .flat_map(|s| sentences(words(s)))
        .flat_map(|s| split(s, chars, lines))
        .collect();
    let shortest = fps.secs_to_frames(SHORTEST_CUE).max(1);
    let mut made: Vec<Made> = groups
        .iter()
        .filter_map(|g| {
            let (first, last) = (g.first()?, g.last()?);
            let start = fps.secs_to_frames(first.start).max(0);
            let words = g
                .iter()
                .enumerate()
                .filter(|(i, w)| *i == 0 || !w.joined)
                .map(|(_, w)| (fps.secs_to_frames(w.start) - start).max(0))
                .collect();
            Some(Made {
                start,
                end: fps.secs_to_frames(last.end).max(start + shortest),
                text: wrap(g, chars).join("\n"),
                words,
                speaker: first.speaker,
            })
        })
        .collect();
    for i in 1..made.len() {
        let next = made[i].start;
        let prev = &mut made[i - 1];
        if prev.end > next - layout.gap {
            prev.end = (next - layout.gap).max(prev.start + 1);
        }
    }
    made
}
