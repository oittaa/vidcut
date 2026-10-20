use super::text::{Tag, Token, render, tokens};
use crate::model::{LetterCase, SpeakerPaint, SubtitleStyle, WordsShow};

pub const POP_SECS: f64 = 0.2;
pub const HIGHLIGHT: &str = "#ffd400";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Piece {
    pub text: String,
    pub italic: bool,
    pub bold: bool,
    pub underline: bool,
    pub color: Option<String>,
    pub hidden: bool,
    pub popped: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub pieces: Vec<Piece>,
    pub spoken: Option<usize>,
    pub step: Option<i64>,
    pub pop: f64,
}

pub fn pieces(text: &str) -> Vec<Piece> {
    let (mut italic, mut bold, mut underline) = (0usize, 0usize, 0usize);
    let mut colors: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for t in tokens(text) {
        match t {
            Token::Text(x) => out.push(Piece {
                text: x,
                italic: italic > 0,
                bold: bold > 0,
                underline: underline > 0,
                color: colors.last().cloned(),
                ..Piece::default()
            }),
            Token::Open(Tag::I) => italic += 1,
            Token::Open(Tag::B) => bold += 1,
            Token::Open(Tag::U) => underline += 1,
            Token::Open(Tag::Color(c)) => colors.push(c),
            Token::Close(Tag::I) => italic = italic.saturating_sub(1),
            Token::Close(Tag::B) => bold = bold.saturating_sub(1),
            Token::Close(Tag::U) => underline = underline.saturating_sub(1),
            Token::Close(Tag::Color(_)) => {
                colors.pop();
            }
        }
    }
    out
}

pub fn cased(text: &str, case: Option<LetterCase>) -> String {
    match case {
        Some(LetterCase::Upper) => text.to_uppercase(),
        Some(LetterCase::Lower) => text.to_lowercase(),
        None => text.to_string(),
    }
}

pub fn cased_markup(text: &str, case: Option<LetterCase>) -> String {
    if case.is_none() {
        return text.to_string();
    }
    let changed: Vec<Token> = tokens(text)
        .into_iter()
        .map(|t| match t {
            Token::Text(x) => Token::Text(cased(&x, case)),
            other => other,
        })
        .collect();
    render(&changed)
}

fn by_word(pieces: Vec<Piece>) -> Vec<(Piece, Option<usize>)> {
    let mut out: Vec<(Piece, Option<usize>)> = Vec::new();
    let mut count = 0usize;
    let mut in_word = false;
    for p in pieces {
        let mut part = String::new();
        let mut part_word: Option<usize> = None;
        for ch in p.text.chars() {
            let word = if ch.is_whitespace() {
                in_word = false;
                None
            } else {
                if !in_word {
                    in_word = true;
                    count += 1;
                }
                Some(count - 1)
            };
            if word != part_word && !part.is_empty() {
                let text = std::mem::take(&mut part);
                out.push((Piece { text, ..p.clone() }, part_word));
            }
            part_word = word;
            part.push(ch);
        }
        if !part.is_empty() {
            out.push((Piece { text: part, ..p }, part_word));
        }
    }
    out
}

pub fn shown(
    text: &str,
    words: &[i64],
    style: &SubtitleStyle,
    frame: i64,
    fps: f64,
    paint: Option<&str>,
) -> Shown {
    let at_once = style.words.is_none() || words.is_empty();
    let paints = style.paints();
    let whole = paint
        .filter(|_| paints == SpeakerPaint::Text || (paints == SpeakerPaint::Spoken && at_once));
    let mut all = pieces(text);
    for p in &mut all {
        p.text = cased(&p.text, style.case);
        if p.color.is_none() {
            p.color = whole.map(String::from);
        }
    }
    let still = Shown {
        pieces: all.clone(),
        spoken: None,
        step: None,
        pop: 1.0,
    };
    let Some(how) = &style.words else {
        return still;
    };
    if words.is_empty() {
        return still;
    }
    let spoken = words.iter().rposition(|&w| w <= frame);
    let show = how.show.unwrap_or_default();
    let color = match (
        paint.filter(|_| paints == SpeakerPaint::Spoken),
        how.color.as_deref(),
        show,
    ) {
        (Some(c), _, _) | (None, Some(c), _) => Some(c.to_string()),
        (None, None, WordsShow::Highlight) => Some(HIGHLIGHT.to_string()),
        (None, None, _) => None,
    };
    let length = (POP_SECS * fps).round().max(1.0) as i64;
    let step = spoken
        .map(|k| frame - words[k])
        .filter(|&t| how.pop.is_some_and(|p| p > 1.0) && t < length);
    let pop = match (step, how.pop) {
        (Some(t), Some(p)) => 1.0 + (p - 1.0) * (1.0 - t as f64 / length as f64).powi(2),
        _ => 1.0,
    };
    let mut out = Vec::new();
    for (mut p, word) in by_word(all) {
        let is_spoken = word.is_some() && word == spoken;
        match show {
            WordsShow::Word if !is_spoken => continue,
            WordsShow::Reveal => p.hidden = word.is_some_and(|w| spoken.is_none_or(|k| w > k)),
            _ => {}
        }
        if is_spoken {
            if let Some(c) = &color {
                p.color = Some(c.clone());
            }
            p.popped = step.is_some();
        }
        out.push(p);
    }
    Shown {
        pieces: out,
        spoken,
        step,
        pop,
    }
}
