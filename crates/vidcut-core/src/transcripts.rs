use std::collections::HashMap;
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

pub fn taken(words: &[Word], start: f64, end: f64) -> Vec<ChangedWord> {
    words
        .iter()
        .filter(|w| covers(start, end, middle(w)))
        .map(|w| ChangedWord {
            text: w.text.clone(),
            start: w.start,
            end: w.end,
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

fn linked(a: &Word, b: &Word) -> bool {
    let overlap = a.end.min(b.end) - a.start.max(b.start);
    let shorter = (a.end - a.start).min(b.end - b.start);
    overlap > 0.0 && overlap * 3.0 >= shorter
}

fn root(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

type Group = (Vec<usize>, Vec<usize>);

fn groups(a: &[Word], b: &[Word]) -> Vec<Group> {
    let n = a.len();
    let mut parent: Vec<usize> = (0..n + b.len()).collect();
    let mut low = 0;
    for (i, w) in a.iter().enumerate() {
        while low < b.len() && b[low].end <= w.start {
            low += 1;
        }
        let mut j = low;
        while j < b.len() && b[j].start < w.end {
            if linked(w, &b[j]) {
                let (x, y) = (root(&mut parent, i), root(&mut parent, n + j));
                parent[x] = y;
            }
            j += 1;
        }
    }
    let mut by_root: HashMap<usize, (Group, f64)> = HashMap::new();
    for (i, w) in a.iter().enumerate() {
        let r = root(&mut parent, i);
        let entry = by_root
            .entry(r)
            .or_insert(((Vec::new(), Vec::new()), w.start));
        entry.0.0.push(i);
        entry.1 = entry.1.min(w.start);
    }
    for (j, w) in b.iter().enumerate() {
        let r = root(&mut parent, n + j);
        let entry = by_root
            .entry(r)
            .or_insert(((Vec::new(), Vec::new()), w.start));
        entry.0.1.push(j);
        entry.1 = entry.1.min(w.start);
    }
    let mut out: Vec<(Group, f64)> = by_root.into_values().collect();
    out.sort_by(|x, y| x.1.total_cmp(&y.1));
    out.into_iter().map(|(g, _)| g).collect()
}

fn bare_text<'a>(words: impl Iterator<Item = &'a Word>) -> String {
    words.map(|w| transcript::bare(&w.text)).collect()
}

pub fn differences(words: &[Word], others: &[&Kept]) -> Vec<Difference> {
    if words.is_empty() {
        return Vec::new();
    }
    let heard: Vec<(&Kept, Vec<Word>)> = others
        .iter()
        .filter_map(|k| Some((*k, transcript::spoken(&k.speech).ok()?)))
        .collect();
    let mut raw: Vec<(usize, usize, f64, f64)> = Vec::new();
    for (_, other) in &heard {
        for (mine, theirs) in groups(words, other) {
            let same = bare_text(mine.iter().map(|&i| &words[i]))
                == bare_text(theirs.iter().map(|&j| &other[j]));
            if same {
                continue;
            }
            let (f, l) = match (mine.first(), mine.last()) {
                (Some(&f), Some(&l)) => (f, l),
                _ => {
                    let at = theirs.first().map_or(0.0, |&j| other[j].start);
                    let before = words.iter().rposition(|w| w.start <= at).unwrap_or(0);
                    (before, before)
                }
            };
            let times = (f..=l)
                .map(|i| &words[i])
                .chain(theirs.iter().map(|&j| &other[j]));
            let (s, e) = times.fold((f64::MAX, f64::MIN), |(s, e), w| {
                (s.min(w.start), e.max(w.end))
            });
            raw.push((f, l, s, e));
        }
    }
    raw.sort_by_key(|x| (x.0, x.1));
    let mut merged: Vec<(usize, usize, f64, f64)> = Vec::new();
    for (f, l, s, e) in raw {
        match merged.last_mut() {
            Some(last) if f <= last.1 => {
                last.1 = last.1.max(l);
                last.2 = last.2.min(s);
                last.3 = last.3.max(e);
            }
            _ => merged.push((f, l, s, e)),
        }
    }
    let mut out = Vec::new();
    for (f, l, mut start, mut end) in merged {
        if f > 0 {
            start = start.max(middle(&words[f - 1]) + 1e-6);
        }
        if let Some(next) = words.get(l + 1) {
            end = end.min(middle(next));
        }
        let mine = bare_text(words[f..=l].iter());
        let mut options: Vec<Alternative> = Vec::new();
        for (k, other) in &heard {
            let theirs: Vec<Word> = other
                .iter()
                .filter(|w| covers(start, end, middle(w)))
                .cloned()
                .collect();
            let text = bare_text(theirs.iter());
            if text == mine {
                continue;
            }
            match options
                .iter_mut()
                .find(|o| bare_text(o.words.iter()) == text)
            {
                Some(o) => o.keys.push(k.key.clone()),
                None => options.push(Alternative {
                    keys: vec![k.key.clone()],
                    words: theirs,
                }),
            }
        }
        if !options.is_empty() {
            out.push(Difference {
                first: f,
                last: l,
                start,
                end,
                options,
            });
        }
    }
    out
}
