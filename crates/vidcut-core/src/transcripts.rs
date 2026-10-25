use std::ops::Range;
use std::sync::Arc;

use crate::editor::Env;
use crate::error::Result;
use crate::model::{ChangedWord, ChosenWords, Media, WordsChange};
use crate::request::FindSpeech;
use crate::transcript::{self, Asked, Said, Speech, Word};

#[derive(Debug, Clone)]
pub struct Kept {
    pub key: String,
    pub made: i64,
    pub speech: Arc<Speech>,
}

pub fn key(speech: &Speech) -> String {
    let settings = format!(
        "{}|{}|{:?}|{}|{}",
        speech.model,
        speech.language.as_deref().unwrap_or(""),
        speech.found,
        speech.sound_db.map(|db| db.to_string()).unwrap_or_default(),
        speech.speakers_by.as_deref().unwrap_or(""),
    );
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in settings.bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

pub fn label(speech: &Speech) -> String {
    let mut parts = vec![speech.model.clone()];
    parts.push(match speech.found {
        FindSpeech::Voice => "speech detector".to_string(),
        FindSpeech::Sound => format!(
            "sound over {} dB",
            speech.sound_db.unwrap_or(transcript::SOUND_DB)
        ),
        FindSpeech::None => "all of the audio".to_string(),
    });
    if let Some(language) = &speech.language {
        parts.push(language.clone());
    }
    if let Some(by) = &speech.speakers_by {
        parts.push(format!("speakers by {by}"));
    }
    parts.join(" · ")
}

pub fn choice(m: &Media, stream: u32) -> Option<&ChosenWords> {
    m.words.iter().find(|c| c.stream == stream)
}

pub fn choice_mut(m: &mut Media, stream: u32) -> &mut ChosenWords {
    match m.words.iter().position(|c| c.stream == stream) {
        Some(i) => &mut m.words[i],
        None => {
            m.words.push(ChosenWords {
                stream,
                main: None,
                changes: Vec::new(),
            });
            m.words.sort_by_key(|c| c.stream);
            m.words
                .iter_mut()
                .find(|c| c.stream == stream)
                .expect("just added")
        }
    }
}

pub fn forget_empty(m: &mut Media) {
    m.words
        .retain(|c| c.main.is_some() || !c.changes.is_empty());
}

pub fn main<'a>(kept: &'a [Kept], choice: Option<&ChosenWords>) -> Option<&'a Kept> {
    let wanted = choice.and_then(|c| c.main.as_deref());
    wanted
        .and_then(|k| kept.iter().find(|t| t.key == k))
        .or_else(|| kept.first())
}

#[derive(Debug, Clone)]
pub struct Chosen {
    pub key: String,
    pub speech: Arc<Speech>,
}

pub fn chosen(kept: &[Kept], choice: Option<&ChosenWords>) -> Option<Chosen> {
    let main = main(kept, choice)?;
    let changes = choice.map_or(&[][..], |c| c.changes.as_slice());
    if changes.is_empty() {
        return Some(Chosen {
            key: main.key.clone(),
            speech: main.speech.clone(),
        });
    }
    let base = transcript::spoken(&main.speech).unwrap_or_default();
    let words = changed(&base, changes);
    let mut speech = (*main.speech).clone();
    speech.said = vec![Said {
        start: words.first().map_or(0.0, |w| w.start),
        end: words.iter().map(|w| w.end).fold(0.0, f64::max),
        text: text(&words),
        words,
        speech: main
            .speech
            .said
            .iter()
            .flat_map(|s| s.speech.iter().copied())
            .collect(),
    }];
    Some(Chosen {
        key: main.key.clone(),
        speech: Arc::new(speech),
    })
}

pub fn read(env: &dyn Env, m: &Media, stream: u32) -> Result<Option<Chosen>> {
    Ok(chosen(&env.transcripts(m, stream)?, choice(m, stream)))
}

pub fn made(env: &dyn Env, m: &Media, stream: u32, asked: &Asked) -> Result<Option<Arc<Speech>>> {
    let kept = env.transcripts(m, stream)?;
    let chosen = chosen(&kept, choice(m, stream)).filter(|c| asked.made(&c.speech));
    Ok(match chosen {
        Some(c) => Some(c.speech),
        None => kept
            .into_iter()
            .find(|k| asked.made(&k.speech))
            .map(|k| k.speech),
    })
}

fn middle(w: &Word) -> f64 {
    (w.start + w.end) / 2.0
}

fn covers(start: f64, end: f64, at: f64) -> bool {
    start <= at && at < end
}

pub fn changed(base: &[Word], changes: &[WordsChange]) -> Vec<Word> {
    let inside = |c: &WordsChange, w: &Word| covers(c.start, c.end, middle(w));
    let mut out: Vec<Word> = base
        .iter()
        .filter(|w| !changes.iter().any(|c| inside(c, w)))
        .cloned()
        .collect();
    for c in changes {
        let speaker = base
            .iter()
            .find(|w| inside(c, w))
            .or_else(|| base.iter().rev().find(|w| w.end <= c.start))
            .and_then(|w| w.speaker);
        out.extend(c.words.iter().map(|w| Word {
            start: w.start,
            end: w.end,
            text: w.text.clone(),
            joined: w.joined,
            speaker,
        }));
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

pub fn put(choice: &mut ChosenWords, start: f64, end: f64, new: Option<WordsChange>) {
    let mut kept = Vec::new();
    for c in std::mem::take(&mut choice.changes) {
        if c.end <= start || c.start >= end {
            kept.push(c);
            continue;
        }
        let part = |a: f64, b: f64| WordsChange {
            start: a,
            end: b,
            words: c
                .words
                .iter()
                .filter(|w| covers(a, b, (w.start + w.end) / 2.0))
                .cloned()
                .collect(),
            from: c.from.clone(),
        };
        if c.start < start {
            kept.push(part(c.start, start));
        }
        if c.end > end {
            kept.push(part(end, c.end));
        }
    }
    kept.extend(new);
    kept.retain(|c| c.end > c.start);
    kept.sort_by(|a, b| a.start.total_cmp(&b.start));
    choice.changes = kept;
}

pub fn taken(words: &[Word], theirs: &[Word], start: f64, end: f64) -> Vec<ChangedWord> {
    let inside: Vec<usize> = (0..words.len())
        .filter(|&i| covers(start, end, middle(&words[i])))
        .collect();
    let (Some(&first), Some(&last)) = (inside.first(), inside.last()) else {
        return Vec::new();
    };
    let picked: Vec<&Word> = counterpart(&align(words, theirs), first, last)
        .into_iter()
        .map(|j| &theirs[j])
        .collect();
    let (Some(a), Some(b)) = (picked.first(), picked.last()) else {
        return Vec::new();
    };
    let after = first.checked_sub(1).map_or(f64::MIN, |i| words[i].end);
    let before = words.get(last + 1).map_or(f64::MAX, |w| w.start);
    let fits =
        a.start >= after && b.end <= before && picked.iter().all(|w| covers(start, end, middle(w)));
    let (mut lo, mut hi) = (start.max(after), end.min(before));
    if hi <= lo {
        (lo, hi) = (start, end);
    }
    let length = b.end - a.start;
    let at = |t: f64| match (fits, length > 0.0) {
        (true, _) => t,
        (false, true) => lo + (t - a.start) / length * (hi - lo),
        (false, false) => lo,
    };
    picked
        .iter()
        .map(|w| ChangedWord {
            text: w.text.clone(),
            start: at(w.start),
            end: at(w.end),
            joined: w.joined,
        })
        .collect()
}

pub fn typed(text: &str, replaced: &[Word], start: f64, end: f64) -> Vec<ChangedWord> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if !replaced.is_empty() && tokens.len() == replaced.len() {
        return tokens
            .iter()
            .zip(replaced)
            .map(|(t, w)| ChangedWord {
                text: t.to_string(),
                start: w.start,
                end: w.end,
                joined: w.joined,
            })
            .collect();
    }
    transcript::spread(text, &[(start, end)])
        .into_iter()
        .map(|w| ChangedWord {
            text: w.text,
            start: w.start,
            end: w.end,
            joined: w.joined,
        })
        .collect()
}

pub fn text(words: &[Word]) -> String {
    let mut out = String::new();
    for w in words {
        if !out.is_empty() && !w.joined {
            out.push(' ');
        }
        out.push_str(&w.text);
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct Difference {
    pub first: usize,
    pub last: usize,
    pub start: f64,
    pub end: f64,
    pub options: Vec<Alternative>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Alternative {
    pub keys: Vec<String>,
    pub words: Vec<Word>,
}

const LOOK: usize = 12;

const NEAR: f64 = 10.0;

#[derive(Debug, Clone, PartialEq)]
enum Piece {
    Same(usize, usize),
    Apart(Range<usize>, Range<usize>),
}

fn align(a: &[Word], b: &[Word]) -> Vec<Piece> {
    let bare =
        |w: &[Word]| -> Vec<String> { w.iter().map(|x| transcript::bare(&x.text)).collect() };
    let (x, y) = (bare(a), bare(b));
    let same = |i: usize, j: usize| x[i] == y[j] && (middle(&a[i]) - middle(&b[j])).abs() <= NEAR;
    let anchored = |i: usize, j: usize| {
        same(i, j) && (i + 1 == a.len() || j + 1 == b.len() || same(i + 1, j + 1))
    };
    let resync = |i: usize, j: usize| {
        (1..=LOOK).find_map(|d| {
            (0..=d)
                .filter(|&k| i + k < a.len() && j + d - k < b.len() && anchored(i + k, j + d - k))
                .min_by_key(|&k| (2 * k).abs_diff(d))
                .map(|k| (k, d - k))
        })
    };
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    let (mut from_i, mut from_j) = (0, 0);
    while i < a.len() && j < b.len() {
        if same(i, j) {
            if from_i < i || from_j < j {
                out.push(Piece::Apart(from_i..i, from_j..j));
            }
            out.push(Piece::Same(i, j));
            (i, j) = (i + 1, j + 1);
            (from_i, from_j) = (i, j);
            continue;
        }
        match resync(i, j) {
            Some((k, l)) => (i, j) = (i + k, j + l),
            None if a[i].start <= b[j].start => i += 1,
            None => j += 1,
        }
    }
    if from_i < a.len() || from_j < b.len() {
        out.push(Piece::Apart(from_i..a.len(), from_j..b.len()));
    }
    out
}

fn counterpart(pieces: &[Piece], first: usize, last: usize) -> Vec<usize> {
    let inside = |i: usize| first <= i && i <= last;
    let mut out = Vec::new();
    let mut before: Option<usize> = None;
    for piece in pieces {
        match piece {
            Piece::Same(i, j) => {
                if inside(*i) {
                    out.push(*j);
                }
                before = Some(*i);
            }
            Piece::Apart(is, js) if is.is_empty() => {
                if inside(before.unwrap_or(0)) {
                    out.extend(js.clone());
                }
            }
            Piece::Apart(is, js) => {
                if is.clone().any(inside) {
                    out.extend(js.clone());
                }
                before = Some(is.end - 1);
            }
        }
    }
    out
}

fn bare_text(words: &[Word]) -> String {
    words.iter().map(|w| transcript::bare(&w.text)).collect()
}

fn bounds(words: &[Word], first: usize, last: usize) -> (f64, f64) {
    let start = match first {
        0 => words[0].start.min(middle(&words[0])),
        f => (middle(&words[f - 1]) + middle(&words[f])) / 2.0,
    };
    let end = match words.get(last + 1) {
        Some(next) => (middle(&words[last]) + middle(next)) / 2.0,
        None => words[last].end.max(middle(&words[last])) + 1e-6,
    };
    (start, end)
}

pub fn differences(words: &[Word], kept: &[&Kept]) -> Vec<Difference> {
    if words.is_empty() {
        return Vec::new();
    }
    let heard: Vec<(&Kept, Vec<Word>, Vec<Piece>)> = kept
        .iter()
        .filter_map(|k| {
            let theirs = transcript::spoken(&k.speech).ok()?;
            let pieces = align(words, &theirs);
            Some((*k, theirs, pieces))
        })
        .collect();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for (_, theirs, pieces) in &heard {
        for piece in pieces {
            let Piece::Apart(is, js) = piece else {
                continue;
            };
            if bare_text(&words[is.clone()]) == bare_text(&theirs[js.clone()]) {
                continue;
            }
            spans.push(match is.is_empty() {
                true => {
                    let at = is.start.saturating_sub(1).min(words.len() - 1);
                    (at, at)
                }
                false => (is.start, is.end - 1),
            });
        }
    }
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (f, l) in spans {
        match merged.last_mut() {
            Some(last) if f <= last.1 => last.1 = last.1.max(l),
            _ => merged.push((f, l)),
        }
    }
    let mut out = Vec::new();
    for (first, last) in merged {
        let mine = bare_text(&words[first..=last]);
        let mut options: Vec<Alternative> = Vec::new();
        for (k, theirs, pieces) in &heard {
            let heard: Vec<Word> = counterpart(pieces, first, last)
                .into_iter()
                .map(|j| theirs[j].clone())
                .collect();
            let text = bare_text(&heard);
            match options.iter_mut().find(|o| bare_text(&o.words) == text) {
                Some(o) => o.keys.push(k.key.clone()),
                None => options.push(Alternative {
                    keys: vec![k.key.clone()],
                    words: heard,
                }),
            }
        }
        if options.iter().any(|o| bare_text(&o.words) != mine) {
            let (start, end) = bounds(words, first, last);
            out.push(Difference {
                first,
                last,
                start,
                end,
                options,
            });
        }
    }
    out
}
