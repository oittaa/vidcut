use crate::model::Place;

const CP1252: [char; 32] = [
    '\u{20AC}', '\u{81}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{8D}', '\u{017D}', '\u{8F}',
    '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{9D}', '\u{017E}', '\u{0178}',
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    Windows1252,
}

impl Encoding {
    pub fn name(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Utf16Le => "UTF-16LE",
            Self::Utf16Be => "UTF-16BE",
            Self::Windows1252 => "Windows-1252",
        }
    }
}

fn utf16(bytes: &[u8], little: bool) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&b| {
            if little {
                u16::from_le_bytes(b)
            } else {
                u16::from_be_bytes(b)
            }
        })
        .collect();
    String::from_utf16_lossy(&units)
}

fn zeros_at(bytes: &[u8], odd: bool) -> usize {
    bytes
        .iter()
        .take(4096)
        .skip(usize::from(odd))
        .step_by(2)
        .filter(|&&b| b == 0)
        .count()
}

pub fn decode(bytes: &[u8]) -> (String, Encoding) {
    let (text, encoding) = if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        (String::from_utf8_lossy(rest).into_owned(), Encoding::Utf8)
    } else if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        (utf16(rest, true), Encoding::Utf16Le)
    } else if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        (utf16(rest, false), Encoding::Utf16Be)
    } else if let Ok(s) = std::str::from_utf8(bytes) {
        (s.to_string(), Encoding::Utf8)
    } else {
        let half = bytes.len().min(4096) / 2;
        if half > 0 && zeros_at(bytes, true) * 3 > half {
            (utf16(bytes, true), Encoding::Utf16Le)
        } else if half > 0 && zeros_at(bytes, false) * 3 > half {
            (utf16(bytes, false), Encoding::Utf16Be)
        } else {
            let s = bytes
                .iter()
                .map(|&b| match b {
                    0x80..=0x9F => CP1252[usize::from(b - 0x80)],
                    _ => char::from(b),
                })
                .collect();
            (s, Encoding::Windows1252)
        }
    };
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(&text);
    (text.replace("\r\n", "\n").replace('\r', "\n"), encoding)
}

pub fn entity(name: &str) -> Option<char> {
    if let Some(n) = name.strip_prefix('#') {
        let code = match n.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => n.parse().ok()?,
        };
        return char::from_u32(code);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{A0}',
        "lrm" => '\u{200E}',
        "rlm" => '\u{200F}',
        _ => return None,
    })
}

const NAMED: &[(&str, &str)] = &[
    ("white", "#ffffff"),
    ("black", "#000000"),
    ("red", "#ff0000"),
    ("lime", "#00ff00"),
    ("green", "#008000"),
    ("blue", "#0000ff"),
    ("yellow", "#ffff00"),
    ("cyan", "#00ffff"),
    ("aqua", "#00ffff"),
    ("magenta", "#ff00ff"),
    ("fuchsia", "#ff00ff"),
    ("gray", "#808080"),
    ("grey", "#808080"),
    ("silver", "#c0c0c0"),
    ("maroon", "#800000"),
    ("olive", "#808000"),
    ("navy", "#000080"),
    ("purple", "#800080"),
    ("teal", "#008080"),
    ("orange", "#ffa500"),
];

pub const PALETTE: &[(&str, &str)] = &[
    ("white", "#ffffff"),
    ("lime", "#00ff00"),
    ("cyan", "#00ffff"),
    ("red", "#ff0000"),
    ("yellow", "#ffff00"),
    ("magenta", "#ff00ff"),
    ("blue", "#0000ff"),
    ("black", "#000000"),
];

pub fn color(value: &str) -> Option<String> {
    let v = value.trim().to_ascii_lowercase();
    if let Some((_, hex)) = NAMED.iter().find(|(n, _)| *n == v) {
        return Some((*hex).to_string());
    }
    if let Some(inner) = v
        .strip_prefix("rgba(")
        .or_else(|| v.strip_prefix("rgb("))
        .and_then(|r| r.strip_suffix(')'))
    {
        let parts: Vec<u8> = inner
            .split(',')
            .take(3)
            .filter_map(|p| p.trim().parse().ok())
            .collect();
        return (parts.len() == 3)
            .then(|| format!("#{:02x}{:02x}{:02x}", parts[0], parts[1], parts[2]));
    }
    let hex = v.strip_prefix('#').unwrap_or(&v);
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        3 | 4 => {
            let d: Vec<char> = hex.chars().collect();
            Some(format!("#{0}{0}{1}{1}{2}{2}", d[0], d[1], d[2]))
        }
        6 | 8 => Some(format!("#{}", &hex[..6])),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tag {
    I,
    B,
    U,
    Color(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Text(String),
    Open(Tag),
    Close(Tag),
}

impl Tag {
    fn name(&self) -> &'static str {
        match self {
            Self::I => "i",
            Self::B => "b",
            Self::U => "u",
            Self::Color(_) => "font",
        }
    }

    pub fn same(&self, other: &Self) -> bool {
        self.name() == other.name()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Srt,
    Vtt,
}

struct Raw<'a> {
    closing: bool,
    name: String,
    rest: &'a str,
    len: usize,
}

fn raw_tag(s: &str) -> Option<Raw<'_>> {
    let end = s.find('>')?;
    let inner = &s[1..end];
    if inner.contains('<') || inner.contains('\n') {
        return None;
    }
    let (closing, body) = match inner.strip_prefix('/') {
        Some(b) => (true, b),
        None => (false, inner),
    };
    let name_end = body
        .find(|c: char| c.is_whitespace() || c == '.')
        .unwrap_or(body.len());
    let name = body[..name_end].to_ascii_lowercase();
    let valid = !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric())
        && name.starts_with(|c: char| c.is_ascii_alphabetic());
    let timestamp = !closing
        && body
            .chars()
            .all(|c| c.is_ascii_digit() || c == ':' || c == '.');
    if !valid && !timestamp {
        return None;
    }
    Some(Raw {
        closing,
        name: if timestamp { String::new() } else { name },
        rest: &body[name_end..],
        len: end + 1,
    })
}

fn attribute(rest: &str, key: &str) -> Option<String> {
    let lower = rest.to_ascii_lowercase();
    let at = lower.find(&format!("{key}="))? + key.len() + 1;
    let v = &rest[at..];
    let value = match v.chars().next()? {
        q @ ('"' | '\'') => v[1..].split(q).next()?,
        _ => v.split(char::is_whitespace).next()?,
    };
    Some(value.to_string())
}

fn ass_place(block: &str) -> Option<Place> {
    let at = block.find("\\an")?;
    match block[at + 3..].chars().next()? {
        '7' | '8' | '9' => Some(Place::Top),
        '4' | '5' | '6' => Some(Place::Middle),
        '1' | '2' | '3' => Some(Place::Bottom),
        _ => None,
    }
}

pub fn markup(raw: &str, dialect: Dialect) -> (String, Option<Place>) {
    let mut tokens = Vec::new();
    let mut text = String::new();
    let mut place = None;
    let mut fonts: Vec<bool> = Vec::new();
    let mut hidden = 0usize;
    let mut rest = raw;
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some(tag) = raw_tag(rest)
        {
            rest = &rest[tag.len..];
            let token = match (tag.name.as_str(), tag.closing) {
                ("i", false) => Some(Token::Open(Tag::I)),
                ("i", true) => Some(Token::Close(Tag::I)),
                ("b", false) => Some(Token::Open(Tag::B)),
                ("b", true) => Some(Token::Close(Tag::B)),
                ("u", false) => Some(Token::Open(Tag::U)),
                ("u", true) => Some(Token::Close(Tag::U)),
                ("font", false) if dialect == Dialect::Srt => {
                    let c = attribute(tag.rest, "color").and_then(|v| color(&v));
                    fonts.push(c.is_some());
                    c.map(|c| Token::Open(Tag::Color(c)))
                }
                ("font", true) if dialect == Dialect::Srt => fonts
                    .pop()
                    .filter(|&kept| kept)
                    .map(|_| Token::Close(Tag::Color(String::new()))),
                ("c", false) if dialect == Dialect::Vtt => {
                    let c = tag
                        .rest
                        .split('.')
                        .find_map(|class| PALETTE.iter().find(|(n, _)| *n == class.trim()));
                    fonts.push(c.is_some());
                    c.map(|(_, hex)| Token::Open(Tag::Color((*hex).to_string())))
                }
                ("c", true) if dialect == Dialect::Vtt => fonts
                    .pop()
                    .filter(|&kept| kept)
                    .map(|_| Token::Close(Tag::Color(String::new()))),
                ("rt" | "rp", false) if dialect == Dialect::Vtt => {
                    hidden += 1;
                    None
                }
                ("rt" | "rp", true) if dialect == Dialect::Vtt => {
                    hidden = hidden.saturating_sub(1);
                    None
                }
                _ => None,
            };
            if let Some(t) = token {
                if !text.is_empty() {
                    tokens.push(Token::Text(std::mem::take(&mut text)));
                }
                tokens.push(t);
            }
            continue;
        }
        if c == '{'
            && dialect == Dialect::Srt
            && rest.starts_with("{\\")
            && let Some(end) = rest.find('}')
        {
            if let Some(p) = ass_place(&rest[..end]) {
                place = Some(p);
            }
            rest = &rest[end + 1..];
            continue;
        }
        if c == '&'
            && let Some(end) = rest[1..].find(';').filter(|&e| e <= 10)
            && let Some(ch) = entity(&rest[1..end + 1])
        {
            if hidden == 0 {
                text.push(ch);
            }
            rest = &rest[end + 2..];
            continue;
        }
        if hidden == 0 {
            text.push(c);
        }
        rest = &rest[c.len_utf8()..];
    }
    if !text.is_empty() {
        tokens.push(Token::Text(text));
    }
    (render(&balance(tokens)), place)
}

pub fn balance(tokens: Vec<Token>) -> Vec<Token> {
    let mut out = Vec::new();
    let mut open: Vec<Tag> = Vec::new();
    for t in tokens {
        match t {
            Token::Open(tag) => {
                open.push(tag.clone());
                out.push(Token::Open(tag));
            }
            Token::Close(tag) => {
                let Some(at) = open.iter().rposition(|o| o.same(&tag)) else {
                    continue;
                };
                let reopen: Vec<Tag> = open.drain(at..).collect();
                for o in reopen.iter().rev() {
                    out.push(Token::Close(o.clone()));
                }
                for o in reopen.into_iter().skip(1) {
                    out.push(Token::Open(o.clone()));
                    open.push(o);
                }
            }
            text => out.push(text),
        }
    }
    for o in open.into_iter().rev() {
        out.push(Token::Close(o));
    }
    let mut cleaned: Vec<Token> = Vec::new();
    for t in out {
        if let (Token::Close(c), Some(Token::Open(o))) = (&t, cleaned.last())
            && c.same(o)
        {
            cleaned.pop();
            continue;
        }
        cleaned.push(t);
    }
    cleaned
}

pub fn render(tokens: &[Token]) -> String {
    let mut s = String::new();
    for t in tokens {
        match t {
            Token::Text(x) => s.push_str(x),
            Token::Open(Tag::Color(c)) => s.push_str(&format!("<font color=\"{c}\">")),
            Token::Open(tag) => s.push_str(&format!("<{}>", tag.name())),
            Token::Close(tag) => s.push_str(&format!("</{}>", tag.name())),
        }
    }
    s
}

pub fn tokens(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some(tag) = raw_tag(rest)
        {
            let t = match (tag.name.as_str(), tag.closing) {
                ("i", false) => Some(Token::Open(Tag::I)),
                ("i", true) => Some(Token::Close(Tag::I)),
                ("b", false) => Some(Token::Open(Tag::B)),
                ("b", true) => Some(Token::Close(Tag::B)),
                ("u", false) => Some(Token::Open(Tag::U)),
                ("u", true) => Some(Token::Close(Tag::U)),
                ("font", false) => attribute(tag.rest, "color")
                    .and_then(|v| color(&v))
                    .map(|c| Token::Open(Tag::Color(c))),
                ("font", true) => Some(Token::Close(Tag::Color(String::new()))),
                _ => None,
            };
            if let Some(t) = t {
                if !plain.is_empty() {
                    out.push(Token::Text(std::mem::take(&mut plain)));
                }
                out.push(t);
                rest = &rest[tag.len..];
                continue;
            }
        }
        plain.push(c);
        rest = &rest[c.len_utf8()..];
    }
    if !plain.is_empty() {
        out.push(Token::Text(plain));
    }
    balance(out)
}

pub fn plain(text: &str) -> String {
    tokens(text)
        .into_iter()
        .filter_map(|t| match t {
            Token::Text(x) => Some(x),
            _ => None,
        })
        .collect()
}

pub fn clean(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let first = lines.iter().position(|l| !l.trim().is_empty());
    let last = lines.iter().rposition(|l| !l.trim().is_empty());
    match (first, last) {
        (Some(a), Some(b)) => lines[a..=b].join("\n"),
        _ => String::new(),
    }
}

pub fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}
