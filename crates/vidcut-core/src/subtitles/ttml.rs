use std::collections::HashMap;

use roxmltree::{Document, Node, ParsingOptions};

use super::text::{self, Tag, Token, escape_xml};
use super::{Read, Timed, clock};
use crate::error::{Error, Result};
use crate::model::{Place, SubtitleStyle};

const TTP: &str = "http://www.w3.org/ns/ttml#parameter";
const TTS: &str = "http://www.w3.org/ns/ttml#styling";
const XML: &str = "http://www.w3.org/XML/1998/namespace";

#[derive(Debug, Clone, Default)]
struct Style {
    italic: Option<bool>,
    bold: Option<bool>,
    underline: Option<bool>,
    color: Option<String>,
}

impl Style {
    fn over(&self, base: &Style) -> Style {
        Style {
            italic: self.italic.or(base.italic),
            bold: self.bold.or(base.bold),
            underline: self.underline.or(base.underline),
            color: self.color.clone().or_else(|| base.color.clone()),
        }
    }

    fn of_attributes(n: Node) -> Style {
        let get = |name: &str| n.attribute((TTS, name));
        Style {
            italic: get("fontStyle").map(|v| v == "italic" || v == "oblique"),
            bold: get("fontWeight").map(|v| v == "bold"),
            underline: get("textDecoration")
                .map(|v| v.contains("underline") && !v.contains("noUnderline")),
            color: get("color").and_then(text::color),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropMode {
    None,
    Ntsc,
    Pal,
}

struct Clock {
    nominal: f64,
    frame_rate: f64,
    sub_rate: f64,
    tick_rate: f64,
    smpte: bool,
    drop: DropMode,
}

impl Clock {
    fn of(root: Node) -> Clock {
        let number = |name: &str| {
            root.attribute((TTP, name))
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|v| *v > 0.0)
        };
        let nominal = number("frameRate").unwrap_or(30.0);
        let multiplier = root
            .attribute((TTP, "frameRateMultiplier"))
            .and_then(|v| {
                let mut it = v.split_whitespace().filter_map(|x| x.parse::<f64>().ok());
                Some(it.next()? / it.next()?)
            })
            .filter(|m| *m > 0.0)
            .unwrap_or(1.0);
        let frame_rate = nominal * multiplier;
        let sub_rate = number("subFrameRate").unwrap_or(1.0);
        let tick_rate = number("tickRate").unwrap_or(if number("frameRate").is_some() {
            frame_rate * sub_rate
        } else {
            1.0
        });
        Clock {
            nominal,
            frame_rate,
            sub_rate,
            tick_rate,
            smpte: root.attribute((TTP, "timeBase")) == Some("smpte"),
            drop: match root.attribute((TTP, "dropMode")) {
                Some("dropNTSC") => DropMode::Ntsc,
                Some("dropPAL") => DropMode::Pal,
                _ => DropMode::None,
            },
        }
    }

    fn time(&self, v: &str) -> Option<f64> {
        let v = v.trim();
        if v.contains(':') {
            let parts: Vec<&str> = v.split(':').collect();
            let h: f64 = parts.first()?.parse().ok()?;
            let m: f64 = parts.get(1)?.parse().ok()?;
            let s: f64 = parts.get(2)?.parse().ok()?;
            let (frames, sub) = match parts.get(3) {
                Some(f) => {
                    let (whole, sub) = f.split_once('.').unwrap_or((f, "0"));
                    (whole.parse::<f64>().ok()?, sub.parse::<f64>().ok()?)
                }
                None => (0.0, 0.0),
            };
            let frames = frames + sub / self.sub_rate;
            if !self.smpte {
                return Some(h * 3600.0 + m * 60.0 + s + frames / self.frame_rate);
            }
            let counted = (3600.0 * h + 60.0 * m + s) * self.nominal + frames;
            let dropped = match self.drop {
                DropMode::None => 0.0,
                DropMode::Ntsc => (h * 54.0 + m - (m / 10.0).floor()) * 2.0,
                DropMode::Pal => (h * 27.0 + (m / 2.0).floor() - (m / 20.0).floor()) * 4.0,
            };
            return Some((counted - dropped) / self.frame_rate);
        }
        let unit_at = v.find(|c: char| c.is_ascii_alphabetic())?;
        let n: f64 = v[..unit_at].parse().ok()?;
        Some(match &v[unit_at..] {
            "h" => n * 3600.0,
            "m" => n * 60.0,
            "s" => n,
            "ms" => n / 1000.0,
            "f" => n / self.frame_rate,
            "t" => n / self.tick_rate,
            _ => return None,
        })
    }
}

struct Sheet<'a> {
    styles: HashMap<&'a str, Node<'a, 'a>>,
    regions: HashMap<&'a str, Node<'a, 'a>>,
    extent: Option<(f64, f64)>,
}

impl<'a> Sheet<'a> {
    fn style(&self, id: &str, depth: usize) -> Style {
        let Some(n) = self.styles.get(id) else {
            return Style::default();
        };
        let own = Style::of_attributes(*n);
        if depth > 8 {
            return own;
        }
        let referenced = self.referenced(*n, depth + 1);
        own.over(&referenced)
    }

    fn referenced(&self, n: Node, depth: usize) -> Style {
        let mut s = Style::default();
        for id in n.attribute("style").unwrap_or("").split_whitespace() {
            s = self.style(id, depth).over(&s);
        }
        s
    }

    fn of(&self, n: Node) -> Style {
        let mut s = self.referenced(n, 0);
        for c in n.children().filter(|c| c.has_tag_name("style")) {
            s = Style::of_attributes(c).over(&s);
        }
        Style::of_attributes(n).over(&s)
    }

    fn named(&self, n: Node<'a, 'a>, name: &str, depth: usize) -> Option<&'a str> {
        if depth > 8 {
            return None;
        }
        n.attribute("style")?.split_whitespace().find_map(|id| {
            let s = self.styles.get(id)?;
            s.attribute((TTS, name))
                .or_else(|| self.named(*s, name, depth + 1))
        })
    }

    fn percent(&self, v: &str, axis: usize) -> Option<f64> {
        let parts: Vec<&str> = v.split_whitespace().collect();
        let p = parts.get(axis)?;
        if let Some(x) = p.strip_suffix('%') {
            return x.parse().ok();
        }
        let px: f64 = p.strip_suffix("px")?.parse().ok()?;
        let (w, h) = self.extent?;
        Some(100.0 * px / if axis == 0 { w } else { h })
    }

    fn place(&self, region: &str) -> Place {
        let Some(r) = self.regions.get(region) else {
            return Place::Bottom;
        };
        let attr = |name: &str| {
            r.attribute((TTS, name))
                .or_else(|| {
                    r.children()
                        .filter(|c| c.has_tag_name("style"))
                        .find_map(|c| c.attribute((TTS, name)))
                })
                .or_else(|| self.named(*r, name, 0))
        };
        let top = attr("origin").and_then(|o| self.percent(o, 1));
        let height = attr("extent").and_then(|e| self.percent(e, 1));
        let Some(top) = top else {
            return match attr("displayAlign") {
                Some("before") => Place::Top,
                Some("center") => Place::Middle,
                _ => Place::Bottom,
            };
        };
        let middle = top + height.unwrap_or(0.0) / 2.0;
        match middle {
            m if m < 40.0 => Place::Top,
            m if m <= 60.0 => Place::Middle,
            _ => Place::Bottom,
        }
    }
}

struct Walk<'a, 'b> {
    sheet: &'b Sheet<'a>,
    clock: Clock,
    cues: Vec<Timed>,
    warnings: Vec<String>,
}

#[derive(Clone)]
struct Scope {
    begin: f64,
    end: Option<f64>,
    style: Style,
    region: Option<String>,
    preserve: bool,
}

impl Walk<'_, '_> {
    fn scope(&self, n: Node, parent: &Scope) -> Scope {
        let begin = n
            .attribute("begin")
            .and_then(|v| self.clock.time(v))
            .map_or(parent.begin, |b| parent.begin + b);
        let end = match (n.attribute("end"), n.attribute("dur")) {
            (Some(e), _) => self.clock.time(e).map(|e| parent.begin + e),
            (None, Some(d)) => self.clock.time(d).map(|d| begin + d),
            (None, None) => parent.end,
        };
        let end = match (end, parent.end) {
            (Some(e), Some(p)) => Some(e.min(p)),
            (e, p) => e.or(p),
        };
        Scope {
            begin,
            end,
            style: self.sheet.of(n).over(&parent.style),
            region: n
                .attribute("region")
                .map(String::from)
                .or_else(|| parent.region.clone()),
            preserve: match n.attribute((XML, "space")) {
                Some(v) => v == "preserve",
                None => parent.preserve,
            },
        }
    }

    fn block(&mut self, n: Node, parent: &Scope) {
        let scope = self.scope(n, parent);
        if n.has_tag_name("p") {
            self.paragraph(n, &scope);
            return;
        }
        for c in n.children().filter(Node::is_element) {
            if c.has_tag_name("div") || c.has_tag_name("p") {
                self.block(c, &scope);
            }
        }
    }

    fn paragraph(&mut self, n: Node, scope: &Scope) {
        let Some(end) = scope.end else {
            self.warnings.push(format!(
                "a paragraph without an end, left out (line {})",
                line(n)
            ));
            return;
        };
        let mut runs = Vec::new();
        inline(self.sheet, n, scope, &mut runs);
        let text = runs_to_text(&runs, scope.preserve);
        if text.trim().is_empty() {
            return;
        }
        self.cues.push(Timed {
            start: scope.begin,
            end,
            text,
            place: scope
                .region
                .as_deref()
                .map_or(Place::Bottom, |r| self.sheet.place(r)),
            words: Vec::new(),
        });
    }
}

fn line(n: Node) -> u32 {
    n.document().text_pos_at(n.range().start).row
}

struct Run {
    text: String,
    style: Style,
    line_break: bool,
}

fn inline(sheet: &Sheet, n: Node, scope: &Scope, runs: &mut Vec<Run>) {
    for c in n.children() {
        if c.is_text() {
            runs.push(Run {
                text: c.text().unwrap_or("").to_string(),
                style: scope.style.clone(),
                line_break: false,
            });
        } else if c.has_tag_name("br") {
            runs.push(Run {
                text: String::new(),
                style: Style::default(),
                line_break: true,
            });
        } else if c.has_tag_name("span") {
            let mut inner = scope.clone();
            inner.style = sheet.of(c).over(&scope.style);
            if let Some(v) = c.attribute((XML, "space")) {
                inner.preserve = v == "preserve";
            }
            inline(sheet, c, &inner, runs);
        }
    }
}

fn collapse(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut space = false;
    for c in t.chars() {
        if c.is_whitespace() {
            space = true;
        } else {
            if space {
                out.push(' ');
                space = false;
            }
            out.push(c);
        }
    }
    if space {
        out.push(' ');
    }
    out
}

fn runs_to_text(runs: &[Run], preserve: bool) -> String {
    let mut lines: Vec<Vec<(String, Style)>> = vec![Vec::new()];
    for r in runs {
        if r.line_break {
            lines.push(Vec::new());
            continue;
        }
        let line = lines.last_mut().expect("a line");
        let mut t = if preserve {
            r.text.clone()
        } else {
            collapse(&r.text)
        };
        let ends_in_space = line.last().is_none_or(|(prev, _)| prev.ends_with(' '));
        if !preserve && ends_in_space {
            t = t.trim_start().to_string();
        }
        if !t.is_empty() {
            line.push((t, r.style.clone()));
        }
    }
    let mut tokens = Vec::new();
    let mut open: Vec<Tag> = Vec::new();
    for (k, mut l) in lines.into_iter().enumerate() {
        if !preserve && let Some((last, _)) = l.last_mut() {
            *last = last.trim_end().to_string();
        }
        if k > 0 {
            tokens.push(Token::Text("\n".into()));
        }
        for (t, s) in &l {
            if t.is_empty() {
                continue;
            }
            let want = wanted(s);
            if want != open {
                for o in open.iter().rev() {
                    tokens.push(Token::Close(o.clone()));
                }
                for w in &want {
                    tokens.push(Token::Open(w.clone()));
                }
                open = want;
            }
            tokens.push(Token::Text(t.clone()));
        }
    }
    for o in open.iter().rev() {
        tokens.push(Token::Close(o.clone()));
    }
    text::clean(&text::render(&text::balance(tokens)))
}

fn wanted(s: &Style) -> Vec<Tag> {
    let mut v = Vec::new();
    if s.bold == Some(true) {
        v.push(Tag::B);
    }
    if s.italic == Some(true) {
        v.push(Tag::I);
    }
    if s.underline == Some(true) {
        v.push(Tag::U);
    }
    if let Some(c) = &s.color
        && c != "#ffffff"
    {
        v.push(Tag::Color(c.clone()));
    }
    v
}

pub fn read(source: &str) -> Result<Read> {
    let options = ParsingOptions {
        allow_dtd: true,
        ..ParsingOptions::default()
    };
    let doc = Document::parse_with_options(source, options)
        .map_err(|e| Error::invalid(format!("not a TTML file: {e}")))?;
    let root = doc.root_element();
    if root.tag_name().name() != "tt" {
        return Err(Error::invalid("not a TTML file: its root isn't <tt>"));
    }
    let extent = root.attribute((TTS, "extent")).and_then(|e| {
        let mut it = e
            .split_whitespace()
            .filter_map(|p| p.strip_suffix("px")?.parse::<f64>().ok());
        Some((it.next()?, it.next()?))
    });
    let mut styles = HashMap::new();
    let mut regions = HashMap::new();
    for n in root.descendants().filter(Node::is_element) {
        let Some(id) = n.attribute((XML, "id")) else {
            continue;
        };
        if n.has_tag_name("style") && n.parent().is_some_and(|p| p.has_tag_name("styling")) {
            styles.insert(id, n);
        } else if n.has_tag_name("region") {
            regions.insert(id, n);
        }
    }
    let sheet = Sheet {
        styles,
        regions,
        extent,
    };
    let mut walk = Walk {
        sheet: &sheet,
        clock: Clock::of(root),
        cues: Vec::new(),
        warnings: Vec::new(),
    };
    let top = Scope {
        begin: 0.0,
        end: None,
        style: Style::default(),
        region: None,
        preserve: root.attribute((XML, "space")) == Some("preserve"),
    };
    let Some(body) = root.children().find(|c| c.has_tag_name("body")) else {
        return Err(Error::invalid("a TTML file without a <body>"));
    };
    walk.block(body, &top);
    let Walk {
        mut cues, warnings, ..
    } = walk;
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    let language = root
        .attribute((XML, "lang"))
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from);
    Ok(Read {
        cues,
        warnings,
        language,
    })
}

fn span_style(tags: &[Tag]) -> String {
    let mut attrs = String::new();
    for t in tags {
        match t {
            Tag::I => attrs.push_str(" tts:fontStyle=\"italic\""),
            Tag::B => attrs.push_str(" tts:fontWeight=\"bold\""),
            Tag::U => attrs.push_str(" tts:textDecoration=\"underline\""),
            Tag::Color(c) => attrs.push_str(&format!(" tts:color=\"{c}\"")),
        }
    }
    attrs
}

fn paragraph(text: &str) -> String {
    let mut out = String::new();
    let mut open: Vec<Tag> = Vec::new();
    let mut in_span = false;
    for t in text::tokens(text) {
        match t {
            Token::Text(x) => {
                for (i, part) in x.split('\n').enumerate() {
                    if i > 0 {
                        if in_span {
                            out.push_str("</span>");
                            in_span = false;
                        }
                        out.push_str("<br/>");
                    }
                    if part.is_empty() {
                        continue;
                    }
                    if !open.is_empty() && !in_span {
                        out.push_str(&format!("<span{}>", span_style(&open)));
                        in_span = true;
                    }
                    out.push_str(&escape_xml(part));
                }
            }
            Token::Open(tag) => {
                if in_span {
                    out.push_str("</span>");
                    in_span = false;
                }
                open.push(tag);
            }
            Token::Close(tag) => {
                if in_span {
                    out.push_str("</span>");
                    in_span = false;
                }
                if let Some(i) = open.iter().rposition(|o| o.same(&tag)) {
                    open.remove(i);
                }
            }
        }
    }
    if in_span {
        out.push_str("</span>");
    }
    out
}

const GRID: f64 = 100_000.0;
const PLACES: [Place; 3] = [Place::Top, Place::Middle, Place::Bottom];

fn rate_attr(frame_rate: Option<f64>) -> String {
    match frame_rate {
        Some(r) if (r - r.round()).abs() < 1e-6 => format!(" ttp:frameRate=\"{}\"", r.round()),
        Some(r) => format!(
            " ttp:frameRate=\"{}\" ttp:frameRateMultiplier=\"1000 1001\"",
            (r * 1001.0 / 1000.0).round()
        ),
        None => String::new(),
    }
}

fn plain_number(x: f64) -> String {
    let r = (x * 1000.0).round() / 1000.0;
    if (r - r.round()).abs() < 1e-9 {
        format!("{}", r.round() as i64)
    } else {
        let s = format!("{r:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

fn percent(units: i64) -> String {
    plain_number(units as f64 / 1000.0)
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn slot(place: Place) -> usize {
    match place {
        Place::Top => 0,
        Place::Middle => 1,
        Place::Bottom => 2,
    }
}

fn name(place: Place) -> &'static str {
    match place {
        Place::Top => "top",
        Place::Middle => "middle",
        Place::Bottom => "bottom",
    }
}

fn stacks(cues: &[Timed]) -> (Vec<usize>, [[bool; 3]; 3]) {
    let lines = |c: &Timed| c.text.split('\n').count();
    let mut order: Vec<usize> = (0..cues.len()).collect();
    order.sort_by(|&a, &b| cues[a].start.total_cmp(&cues[b].start));
    let mut stacked = vec![0; cues.len()];
    let mut together = [[false; 3]; 3];
    let mut showing: Vec<usize> = Vec::new();
    for i in order {
        let c = &cues[i];
        showing.retain(|&j| cues[j].end > c.start);
        stacked[i] = lines(c);
        for &j in &showing {
            let (a, b) = (slot(cues[j].place), slot(c.place));
            if a == b {
                stacked[i] += lines(&cues[j]);
            } else {
                together[a.min(b)][a.max(b)] = true;
            }
        }
        showing.push(i);
    }
    (stacked, together)
}

struct Region {
    top: i64,
    height: i64,
    align: &'static str,
}

fn layout(cues: &[Timed], style: &SubtitleStyle) -> ([Region; 3], Vec<String>) {
    let line = style.text_fraction() * SubtitleStyle::LINE;
    let (stacked, together) = stacks(cues);
    let mut need = [0.0f64; 3];
    for (c, &n) in cues.iter().zip(&stacked) {
        let s = slot(c.place);
        need[s] = need[s].max(n as f64 * line);
    }
    let anchors = PLACES.map(|p| style.anchor(p));
    let mut h = [0, 1, 2].map(|s| {
        let (at, above) = anchors[s];
        let below = if above < 1.0 {
            (1.0 - at) / (1.0 - above)
        } else {
            f64::INFINITY
        };
        let fits = below.min(if above > 0.0 {
            at / above
        } else {
            f64::INFINITY
        });
        let tilt = 0.5 - above;
        let read = match PLACES[s] {
            Place::Top if tilt > 0.0 => (0.399 - at) / tilt,
            Place::Bottom if tilt < 0.0 => (at - 0.601) / -tilt,
            _ => f64::INFINITY,
        };
        if need[s] <= read {
            read.min(fits)
        } else {
            fits
        }
    });
    for (a, b) in [(0, 1), (1, 2), (0, 2)] {
        if !together[a][b] {
            continue;
        }
        let (ka, kb) = (1.0 - anchors[a].1, anchors[b].1);
        let gap = anchors[b].0 - anchors[a].0 - 1.0 / GRID;
        let want = ka * h[a] + kb * h[b];
        if want <= gap {
            continue;
        }
        let kept = [need[a].min(h[a]), need[b].min(h[b])];
        let firm = ka * kept[0] + kb * kept[1];
        let (fa, fb) = if firm <= gap {
            let k = (gap - firm) / (want - firm);
            (
                kept[0] + k * (h[a] - kept[0]),
                kept[1] + k * (h[b] - kept[1]),
            )
        } else {
            let k = gap.max(0.0) / firm;
            (kept[0] * k, kept[1] * k)
        };
        h[a] = fa;
        h[b] = fb;
    }
    let regions = [0, 1, 2].map(|s| {
        let (at, above) = anchors[s];
        let y0 = at - above * h[s];
        let top = (y0 * GRID - 1e-6).ceil().max(0.0) as i64;
        let bottom = ((y0 + h[s]) * GRID + 1e-6).floor().min(GRID) as i64;
        Region {
            top,
            height: (bottom - top).max(0),
            align: if above < 0.25 {
                "before"
            } else if above > 0.75 {
                "after"
            } else {
                "center"
            },
        }
    });
    let mut warnings = Vec::new();
    for (s, region) in regions.iter().enumerate() {
        let room = region.height as f64 / GRID;
        let over: Vec<&Timed> = cues
            .iter()
            .zip(&stacked)
            .filter(|&(c, &n)| slot(c.place) == s && n as f64 * line > room + 2.0 / GRID)
            .map(|(c, _)| c)
            .collect();
        if let Some(first) = over.iter().min_by(|a, b| a.start.total_cmp(&b.start)) {
            warnings.push(format!(
                "{} cue(s) at the {} need more lines than the IMSC1 file's region there holds \
                 ({}), the first at {}; players may cut them off",
                over.len(),
                name(PLACES[s]),
                (room / line + 1e-9).floor(),
                clock(first.start, '.')
            ));
        }
    }
    (regions, warnings)
}

fn style_attrs(style: &SubtitleStyle) -> String {
    let font = match &style.font {
        Some(f) if f.contains(char::is_whitespace) => {
            format!("'{}',proportionalSansSerif", escape_xml(f))
        }
        Some(f) => format!("{},proportionalSansSerif", escape_xml(f)),
        None => "proportionalSansSerif".into(),
    };
    let mut attrs = format!(
        "tts:color=\"{}\" tts:fontFamily=\"{font}\" tts:fontSize=\"{}%\" tts:lineHeight=\"{}%\" \
         tts:textAlign=\"center\"",
        hex(style.ink()),
        plain_number(style.text_fraction() * 10_000.0),
        plain_number(SubtitleStyle::LINE * 100.0),
    );
    if style.bold == Some(true) {
        attrs.push_str(" tts:fontWeight=\"bold\"");
    }
    if let Some((rgb, size)) = style.stroke() {
        attrs.push_str(&format!(
            " tts:textOutline=\"{} {}%\"",
            hex(rgb),
            plain_number(size * 100.0)
        ));
    }
    attrs
}

fn background(style: &SubtitleStyle) -> Option<String> {
    let (rgb, opacity) = style.backing()?;
    let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
    Some(if alpha == 255 {
        hex(rgb)
    } else {
        format!("{}{alpha:02x}", hex(rgb))
    })
}

pub fn write(
    cues: &[Timed],
    language: Option<&str>,
    frame_rate: Option<f64>,
    style: &SubtitleStyle,
) -> (String, Vec<String>) {
    let (placed, warnings) = layout(cues, style);
    let inset = (style.inset() * GRID).round() as i64;
    let regions: String = [Place::Bottom, Place::Middle, Place::Top]
        .into_iter()
        .map(|p| {
            let r = &placed[slot(p)];
            format!(
                "      <region xml:id=\"{}\" tts:origin=\"{}% {}%\" tts:extent=\"{}% {}%\" \
                 tts:displayAlign=\"{}\"/>\n",
                name(p),
                percent(inset),
                percent(r.top),
                percent(GRID as i64 - 2 * inset),
                percent(r.height),
                r.align
            )
        })
        .collect();
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<tt xml:lang=\"{}\" \
         xmlns=\"http://www.w3.org/ns/ttml\" xmlns:ttp=\"{TTP}\" xmlns:tts=\"{TTS}\" \
         xmlns:ittp=\"http://www.w3.org/ns/ttml/profile/imsc1#parameter\" \
         ttp:profile=\"http://www.w3.org/ns/ttml/profile/imsc1/text\" \
         ttp:timeBase=\"media\"{} ttp:cellResolution=\"100 100\">\n  <head>\n    <styling>\n      \
         <style xml:id=\"s\" {}/>\n    </styling>\n    <layout>\n{regions}    </layout>\n  \
         </head>\n  <body style=\"s\">\n    <div>\n",
        escape_xml(language.unwrap_or("")),
        rate_attr(frame_rate),
        style_attrs(style),
    );
    let background = background(style);
    for c in cues {
        let body = paragraph(&super::cased_markup(&c.text, style.case));
        let text = match &background {
            Some(color) => format!("<span tts:backgroundColor=\"{color}\">{body}</span>"),
            None => body,
        };
        out.push_str(&format!(
            "      <p begin=\"{}\" end=\"{}\" region=\"{}\">{text}</p>\n",
            clock(c.start, '.'),
            clock(c.end, '.'),
            name(c.place)
        ));
    }
    out.push_str("    </div>\n  </body>\n</tt>\n");
    (out, warnings)
}
