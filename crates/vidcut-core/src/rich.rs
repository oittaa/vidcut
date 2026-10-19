use crate::subtitles::color;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Look {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub color: Option<String>,
    pub face: Option<String>,
    pub size: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub text: String,
    pub look: Look,
}

#[derive(Debug, Clone, PartialEq)]
enum Open {
    Bold,
    Italic,
    Underline,
    Font {
        color: Option<String>,
        face: Option<String>,
        size: Option<f64>,
    },
}

impl Open {
    fn name(&self) -> &'static str {
        match self {
            Self::Bold => "b",
            Self::Italic => "i",
            Self::Underline => "u",
            Self::Font { .. } => "font",
        }
    }
}

fn attribute(rest: &str, key: &str) -> Option<String> {
    let lower = rest.to_ascii_lowercase();
    let wanted = format!("{key}=");
    let mut from = 0;
    let at = loop {
        let i = from + lower[from..].find(&wanted)?;
        if i == 0 || lower[..i].ends_with(char::is_whitespace) {
            break i + wanted.len();
        }
        from = i + wanted.len();
    };
    let v = &rest[at..];
    let value = match v.chars().next()? {
        q @ ('"' | '\'') => v[1..].split(q).next()?,
        _ => v.split(char::is_whitespace).next()?,
    };
    Some(value.to_string())
}

fn size_of(raw: &str) -> Option<f64> {
    let t = raw.trim();
    let (number, scale) = match t.strip_suffix('%') {
        Some(n) => (n.trim(), 0.01),
        None => (t, 1.0),
    };
    let v = number.parse::<f64>().ok()? * scale;
    (v.is_finite() && v > 0.0 && v <= 0.5).then_some(v)
}

fn tag(s: &str) -> Option<(bool, Option<Open>, usize)> {
    let end = s.find('>')?;
    let inner = &s[1..end];
    if inner.contains('<') || inner.contains('\n') {
        return None;
    }
    let (closing, body) = match inner.strip_prefix('/') {
        Some(b) => (true, b.trim()),
        None => (false, inner),
    };
    let name_end = body.find(char::is_whitespace).unwrap_or(body.len());
    let name = body[..name_end].to_ascii_lowercase();
    let rest = &body[name_end..];
    let open = match name.as_str() {
        "b" => Open::Bold,
        "i" => Open::Italic,
        "u" => Open::Underline,
        "font" => Open::Font {
            color: attribute(rest, "color").and_then(|c| color(&c)),
            face: attribute(rest, "face")
                .map(|f| f.trim().to_string())
                .filter(|f| !f.is_empty()),
            size: attribute(rest, "size").and_then(|s| size_of(&s)),
        },
        _ => return None,
    };
    if closing {
        return (rest.trim().is_empty()).then_some((true, Some(open), end + 1));
    }
    Some((false, Some(open), end + 1))
}

fn entity(s: &str) -> Option<(char, usize)> {
    for (name, c) in [
        ("&lt;", '<'),
        ("&gt;", '>'),
        ("&amp;", '&'),
        ("&quot;", '"'),
    ] {
        if s.starts_with(name) {
            return Some((c, name.len()));
        }
    }
    None
}

fn look_of(stack: &[Open]) -> Look {
    let mut look = Look::default();
    for o in stack {
        match o {
            Open::Bold => look.bold = true,
            Open::Italic => look.italic = true,
            Open::Underline => look.underline = true,
            Open::Font { color, face, size } => {
                if color.is_some() {
                    look.color.clone_from(color);
                }
                if face.is_some() {
                    look.face.clone_from(face);
                }
                if size.is_some() {
                    look.size = *size;
                }
            }
        }
    }
    look
}

pub fn spans(text: &str) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut run = String::new();
    let mut rest = text;
    let flush = |run: &mut String, stack: &[Open], out: &mut Vec<Span>| {
        if run.is_empty() {
            return;
        }
        let look = look_of(stack);
        match out.last_mut() {
            Some(last) if last.look == look => last.text.push_str(run),
            _ => out.push(Span {
                text: run.clone(),
                look,
            }),
        }
        run.clear();
    };
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some((closing, Some(open), len)) = tag(rest)
        {
            flush(&mut run, &stack, &mut out);
            if closing {
                if let Some(i) = stack.iter().rposition(|o| o.name() == open.name()) {
                    stack.remove(i);
                }
            } else {
                stack.push(open);
            }
            rest = &rest[len..];
            continue;
        }
        if c == '&'
            && let Some((ch, len)) = entity(rest)
        {
            run.push(ch);
            rest = &rest[len..];
            continue;
        }
        run.push(c);
        rest = &rest[c.len_utf8()..];
    }
    flush(&mut run, &stack, &mut out);
    out
}

pub fn plain(text: &str) -> String {
    spans(text).into_iter().map(|s| s.text).collect()
}

pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn lines(text: &str) -> Vec<Vec<Span>> {
    let mut out: Vec<Vec<Span>> = vec![Vec::new()];
    for span in spans(text) {
        let mut parts = span.text.split('\n').peekable();
        while let Some(part) = parts.next() {
            if !part.is_empty() {
                out.last_mut().expect("one line at least").push(Span {
                    text: part.to_string(),
                    look: span.look.clone(),
                });
            }
            if parts.peek().is_some() {
                out.push(Vec::new());
            }
        }
    }
    out
}

pub fn largest(line: &[Span], base: f64) -> f64 {
    line.iter()
        .map(|s| s.look.size.unwrap_or(base))
        .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))))
        .unwrap_or(base)
}

fn number(v: f64) -> String {
    let s = format!("{v:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub fn write(line: &[Span], base: &Look) -> String {
    let mut out = String::new();
    for s in line {
        let l = &s.look;
        let mut font = String::new();
        if l.color.is_some() && l.color != base.color {
            font.push_str(&format!(" color=\"{}\"", l.color.as_deref().unwrap_or("")));
        }
        if l.face.is_some() && l.face != base.face {
            font.push_str(&format!(" face=\"{}\"", l.face.as_deref().unwrap_or("")));
        }
        if let Some(size) = l.size.filter(|_| l.size != base.size) {
            font.push_str(&format!(" size=\"{}\"", number(size)));
        }
        let mut open = String::new();
        let mut close = String::new();
        for (on, base_on, name) in [
            (l.bold, base.bold, "b"),
            (l.italic, base.italic, "i"),
            (l.underline, base.underline, "u"),
        ] {
            if on && !base_on {
                open.push_str(&format!("<{name}>"));
                close.insert_str(0, &format!("</{name}>"));
            }
        }
        if !font.is_empty() {
            open.push_str(&format!("<font{font}>"));
            close.insert_str(0, "</font>");
        }
        out.push_str(&open);
        out.push_str(&escape(&s.text));
        out.push_str(&close);
    }
    out
}
