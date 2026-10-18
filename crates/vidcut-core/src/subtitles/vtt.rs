use super::text::{self, Dialect, PALETTE, Tag, Token};
use super::{Read, Timed, clock, srt};
use crate::error::{Error, Result};
use crate::model::Place;

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
    for block in rest.split(|l| l.trim().is_empty()) {
        let Some(first) = block.first() else { continue };
        if first.starts_with("NOTE") || first.starts_with("STYLE") || first.starts_with("REGION") {
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
        let (body, _) = text::markup(&block[at + 1..].join("\n"), Dialect::Vtt);
        cues.push(Timed {
            start,
            end,
            text: text::clean(&body),
            place: place(&settings),
        });
    }
    Ok(Read { cues, warnings })
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
            Token::Open(Tag::Color(c)) => match PALETTE.iter().find(|(_, hex)| *hex == c) {
                Some((name, _)) => s.push_str(&format!("<c.{name}>")),
                None => s.push_str("<c>"),
            },
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

pub fn write(cues: &[Timed]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for c in cues {
        let settings = match c.place {
            Place::Top => " line:0",
            Place::Middle => " line:50%",
            Place::Bottom => "",
        };
        out.push_str(&format!(
            "{} --> {}{settings}\n{}\n\n",
            clock(c.start, '.'),
            clock(c.end, '.'),
            body(&c.text)
        ));
    }
    out
}
