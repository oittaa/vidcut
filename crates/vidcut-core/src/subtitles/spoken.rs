use super::text::{Tag, Token, render, tokens};
use crate::model::{LetterCase, SpeakerPaint, SubtitleStyle, WordsShow};

pub const POP_SECS: f64 = 0.2;
pub const HIGHLIGHT: &str = "#ffd400";

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Piece {
    pub text: String,
    pub italic: bool,
    pub bold: bool,
    pub underline: bool,
    pub color: Option<String>,
    pub hidden: bool,
    pub popped: bool,
    pub face: Option<String>,
    pub size_scale: Option<u32>,
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

fn word_bold(inherited: bool, word: Option<&SubtitleStyle>) -> bool {
    word.and_then(|w| w.bold).unwrap_or(inherited)
}

fn paint_word(
    p: &mut Piece,
    word: Option<usize>,
    style: &SubtitleStyle,
    word_styles: &[Option<SubtitleStyle>],
) {
    let over = word
        .and_then(|i| word_styles.get(i))
        .and_then(|s| s.as_ref());
    if let Some(over) = over {
        if over.case.is_some() {
            p.text = cased(&p.text, over.case);
        }
        if over.font.is_some() {
            p.face = over.font.clone();
        }
        if let Some(size) = over.size {
            let cue = style.text_fraction();
            p.size_scale = (cue > 0.0).then_some((size / cue * 1000.0).round() as u32);
        }
        if over.color.is_some() {
            let [r, g, b] = style.for_cue(Some(over)).ink();
            p.color = Some(format!("#{r:02x}{g:02x}{b:02x}"));
        }
    }
    p.bold = word_bold(p.bold, over);
}

pub fn parts(text: &str) -> Vec<(Piece, Option<usize>)> {
    by_word(pieces(text))
}

fn font_open(cue: &SubtitleStyle, over: Option<&SubtitleStyle>) -> Option<String> {
    let over = over.filter(|s| s.color.is_some() || s.font.is_some())?;
    let mut attrs = String::new();
    if let Some(font) = &over.font {
        attrs.push_str(&format!(" face=\"{font}\""));
    }
    if over.color.is_some() {
        let [r, g, b] = cue.for_cue(Some(over)).ink();
        attrs.push_str(&format!(" color=\"#{r:02x}{g:02x}{b:02x}\""));
    }
    Some(format!("<font{attrs}>"))
}

pub fn marked(text: &str, cue: &SubtitleStyle, words: &[Option<SubtitleStyle>]) -> String {
    if words.iter().all(|s| s.is_none()) {
        return text.to_string();
    }
    let mut out = String::new();
    let mut open_bold = false;
    let mut font: Option<String> = None;
    for (piece, word) in parts(text) {
        let over = word.and_then(|i| words.get(i)).and_then(|s| s.as_ref());
        let bold = word_bold(piece.bold, over);
        let next_font = font_open(cue, over);
        if next_font != font || bold != open_bold {
            if font.is_some() {
                out.push_str("</font>");
            }
            if bold != open_bold {
                out.push_str(if bold { "<b>" } else { "</b>" });
                open_bold = bold;
            }
            if let Some(tag) = &next_font {
                out.push_str(tag);
            }
            font = next_font;
        }
        let case = over.and_then(|s| s.case).or(cue.case);
        out.push_str(&cased(&piece.text, case));
    }
    if font.is_some() {
        out.push_str("</font>");
    }
    if open_bold {
        out.push_str("</b>");
    }
    out
}

pub fn shown(
    text: &str,
    words: &[i64],
    word_styles: &[Option<SubtitleStyle>],
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
        if style.bold == Some(true) {
            p.bold = true;
        }
        if p.color.is_none() {
            p.color = whole.map(String::from);
        }
    }
    let still_pieces = if word_styles.iter().any(|s| s.is_some()) {
        by_word(all.clone())
            .into_iter()
            .map(|(mut p, word)| {
                paint_word(&mut p, word, style, word_styles);
                p
            })
            .collect()
    } else {
        all.clone()
    };
    let still = Shown {
        pieces: still_pieces,
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
        paint_word(&mut p, word, style, word_styles);
        out.push(p);
    }
    Shown {
        pieces: out,
        spoken,
        step,
        pop,
    }
}

impl std::hash::Hash for Shown {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        let Shown {
            pieces,
            spoken,
            step,
            pop,
        } = self;
        (pieces, spoken, step, crate::model::float_bits(*pop)).hash(h);
    }
}

impl Eq for Shown {}
