use std::path::PathBuf;

use serde::Serialize;

use crate::request::SpeechModel;
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Timing {
    Words,
    Phrases,
    Pauses,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Speech {
    pub model: String,
    pub device: String,
    pub language: Option<String>,
    pub timing: Timing,
    pub pieces: usize,
    pub longest_piece: f64,
    pub said: Vec<Said>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Said {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub words: Vec<Word>,
    pub speech: Vec<(f64, f64)>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Word {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub joined: bool,
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
            }
        })
        .collect()
}

fn is_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF | 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x3FFFF)
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
        if now.last().is_some_and(|l| w.start - l.end >= PAUSE) {
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
            Some(Made {
                start,
                end: fps.secs_to_frames(last.end).max(start + shortest),
                text: wrap(g, chars).join("\n"),
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
