use super::text::{self, Dialect, PALETTE, Tag, Token};
use super::{Read, Timed, clock, srt};
use crate::error::{Error, Result};
use crate::model::{Place, SubtitleAlign, SubtitleStyle};

fn place(settings: &str) -> Place {
    let Some(line) = settings
        .split_whitespace()
        .find_map(|s| s.strip_prefix("line:"))
    else {
        return Place::Bottom;
    };
    let value = line.split(',').next().unwrap_or("");
    if let Some(pct) = value.strip_suffix('%').and_then(|v| v.parse::<f64>().ok()) {
        return match pct {
            p if p < 33.0 => Place::Top,
            p if p <= 66.0 => Place::Middle,
            _ => Place::Bottom,
        };
    }
    match value.parse::<i64>() {
        Ok(n) if n >= 0 => Place::Top,
        _ => Place::Bottom,
    }
}

pub fn read(source: &str) -> Result<Read> {
    let mut lines = source.lines();
    let header = lines.next().unwrap_or("");
    if !header.starts_with("WEBVTT") {
        return Err(Error::invalid(
            "not a WebVTT file: it doesn't start with WEBVTT",
        ));
    }
    let rest: Vec<&str> = lines.collect();
    let mut cues = Vec::new();
    let mut warnings = Vec::new();
    let mut classes = Vec::new();
    for block in rest.split(|l| l.trim().is_empty()) {
        let Some(first) = block.first() else { continue };
        if first.starts_with("STYLE") {
            classes.extend(style_colors(&block[1..].join("\n")));
            continue;
        }
        if first.starts_with("NOTE") || first.starts_with("REGION") {
            continue;
        }
        let Some(at) = block.iter().position(|l| l.contains("-->")) else {
            warnings.push(format!("a block without times, left out: {first}"));
            continue;
        };
        let Some((start, end)) = srt::timing(block[at]) else {
            warnings.push(format!("times it can't read, left out: {}", block[at]));
            continue;
        };
        let settings = block[at]
            .split_once("-->")
            .map(|(_, b)| b.split_whitespace().skip(1).collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        let raw = block[at + 1..].join("\n");
        let (body, _) = text::markup(&raw, Dialect::Vtt, &classes);
        let text = text::clean(&body);
        let words = word_times(&raw, &classes, (start, end), &text);
        cues.push(Timed {
            start,
            end,
            text,
            place: place(&settings),
            words,
            speaker: text::voice(&raw),
            style: None,
            word_styles: Vec::new(),
        });
    }
    Ok(Read {
        cues,
        warnings,
        language: None,
    })
}

fn word_times(raw: &str, classes: &[(String, String)], span: (f64, f64), text: &str) -> Vec<f64> {
    let (lo, hi) = if span.0 <= span.1 {
        (span.0, span.1)
    } else {
        (span.1, span.0)
    };
    let words = super::words(text);
    let mut known = vec![None; words.len()];
    let mut from = 0;
    while let Some(i) = raw[from..].find('<') {
        let at = from + i;
        from = at + 1;
        let Some(close) = raw[at..].find('>') else {
            break;
        };
        let Some(t) = srt::time(&raw[at + 1..at + close]).filter(|_| {
            raw[at + 1..at + close]
                .chars()
                .all(|c| c.is_ascii_digit() || c == ':' || c == '.')
        }) else {
            continue;
        };
        let (before, _) = text::markup(&raw[..at], Dialect::Vtt, classes);
        let (after, _) = text::markup(&raw[at + close + 1..], Dialect::Vtt, classes);
        let (before, after) = (text::plain(&before), text::plain(&after));
        let inside = before.ends_with(|c: char| !c.is_whitespace())
            && after.starts_with(|c: char| !c.is_whitespace());
        if inside {
            continue;
        }
        if let Some(k) = known
            .get_mut(before.split_whitespace().count())
            .filter(|k| k.is_none())
        {
            *k = Some(t.clamp(lo, hi));
        }
    }
    if known.iter().all(Option::is_none) {
        return Vec::new();
    }
    let weights: Vec<f64> = words
        .iter()
        .map(|w| w.chars().count() as f64 + 1.0)
        .collect();
    super::fill_times(&known, &weights, lo, hi)
}

fn style_colors(css: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = css;
    while let Some(at) = rest.find("::cue(") {
        rest = &rest[at + 6..];
        let Some(close) = rest.find(')') else { break };
        let selector = rest[..close].trim();
        let (Some(open), Some(end)) = (rest.find('{'), rest.find('}')) else {
            break;
        };
        if open < end
            && let Some(class) = selector.strip_prefix('.')
            && !class.contains(['.', ' ', ':'])
        {
            let color = rest[open + 1..end].split(';').find_map(|d| {
                let (k, v) = d.split_once(':')?;
                (k.trim() == "color").then(|| text::color(v)).flatten()
            });
            if let Some(c) = color {
                out.push((class.to_string(), c));
            }
        }
        rest = &rest[end.max(close) + 1..];
    }
    out
}

fn class_of(hex: &str) -> String {
    match PALETTE.iter().find(|(_, h)| *h == hex) {
        Some((name, _)) => (*name).to_string(),
        None => format!("c{}", hex.trim_start_matches('#')),
    }
}

fn body(text: &str) -> String {
    let mut s = String::new();
    for t in text::tokens(text) {
        match t {
            Token::Text(x) => {
                for c in x.chars() {
                    match c {
                        '&' => s.push_str("&amp;"),
                        '<' => s.push_str("&lt;"),
                        '>' => s.push_str("&gt;"),
                        _ => s.push(c),
                    }
                }
            }
            Token::Open(Tag::Color(c)) => s.push_str(&format!("<c.{}>", class_of(&c))),
            Token::Close(Tag::Color(_)) => s.push_str("</c>"),
            Token::Open(Tag::I) => s.push_str("<i>"),
            Token::Close(Tag::I) => s.push_str("</i>"),
            Token::Open(Tag::B) => s.push_str("<b>"),
            Token::Close(Tag::B) => s.push_str("</b>"),
            Token::Open(Tag::U) => s.push_str("<u>"),
            Token::Close(Tag::U) => s.push_str("</u>"),
        }
    }
    s
}

fn percent(v: f64) -> String {
    let p = v * 100.0;
    let s = format!("{p:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub fn settings(place: Place, look: &SubtitleStyle) -> String {
    let mut parts = Vec::new();
    match look.position {
        Some([x, y]) => {
            let edge = match look.anchor(place).1 {
                a if a <= 0.0 => "",
                a if a >= 1.0 => ",end",
                _ => ",center",
            };
            parts.push(format!("position:{}%", percent(x)));
            parts.push(format!("line:{}%{edge}", percent(y)));
        }
        None => match place {
            Place::Top => parts.push("line:0".into()),
            Place::Middle => parts.push("line:50%,center".into()),
            Place::Bottom => {}
        },
    }
    match look.line_align() {
        SubtitleAlign::Left => parts.push("align:left".into()),
        SubtitleAlign::Right => parts.push("align:right".into()),
        SubtitleAlign::Center => {}
    }
    parts.join(" ")
}

pub fn write(cues: &[Timed], style: &SubtitleStyle) -> String {
    let mut out = String::from("WEBVTT\n\n");
    let mut colors: Vec<String> = Vec::new();
    for c in cues {
        let look = style.for_cue(c.style.as_ref());
        let text = super::marked(&c.text, &look, &c.word_styles);
        for t in text::tokens(&text) {
            if let Token::Open(Tag::Color(hex)) = t
                && !PALETTE.iter().any(|(_, h)| *h == hex)
                && !colors.contains(&hex)
            {
                colors.push(hex);
            }
        }
    }
    if !colors.is_empty() {
        out.push_str("STYLE\n");
        for hex in &colors {
            out.push_str(&format!("::cue(.{}) {{ color: {hex}; }}\n", class_of(hex)));
        }
        out.push('\n');
    }
    for c in cues {
        let look = style.for_cue(c.style.as_ref());
        let settings = match settings(c.place, &look) {
            s if s.is_empty() => s,
            s => format!(" {s}"),
        };
        let voice = c
            .speaker
            .as_deref()
            .map(|n| format!("<v {}>", text::escape_xml(n).replace('\n', " ")))
            .unwrap_or_default();
        out.push_str(&format!(
            "{} --> {}{settings}\n{voice}{}\n\n",
            clock(c.start, '.'),
            clock(c.end, '.'),
            body(&super::marked(&c.text, &look, &c.word_styles))
        ));
    }
    out
}
