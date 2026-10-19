use serde_json::{Value, json};

use super::{EDITOR, VIDCUT};
use crate::model::{
    SubtitleBackground, SubtitleOutline, SubtitleStyle, Title, TitleAlign, TitleKind, TitleShadow,
    TitleStyle,
};
use crate::rich::{self, Look, Span};

pub const STROKE_PER_TEXT: f64 = 160.0;
pub const UNKNOWN_HEIGHT: u32 = 1080;
const USUAL_FAMILY: &str = "Open Sans";

const HEAD: &str = "<!DOCTYPE HTML PUBLIC \"-//W3C//DTD HTML 4.0//EN\" \"http://www.w3.org/TR/REC-html40/strict.dtd\">\n<html><head><meta name=\"qrichtext\" content=\"1\" /><style type=\"text/css\">\np, li { white-space: pre-wrap; }\n</style></head><body style=\" font-family:'MS Shell Dlg 2'; font-size:8.25pt; font-weight:400; font-style:normal;\">\n";

fn kind_of(t: &Title) -> (&'static str, &'static str) {
    match (t.kind, t.align()) {
        (TitleKind::Text, _) => ("Rich", "Text"),
        (TitleKind::LowerThird, TitleAlign::Left) => ("Rich Lower Left", "Left Lower Third"),
        (TitleKind::LowerThird, TitleAlign::Middle) => ("Rich Lower Middle", "Middle Lower Third"),
        (TitleKind::LowerThird, TitleAlign::Right) => ("Rich Lower Right", "Right Lower Third"),
        (TitleKind::Scroll, _) => ("Scroll", "Scroll"),
    }
}

pub fn name(t: &Title) -> &'static str {
    kind_of(t).1
}

fn composite_opacity(e: &Value) -> Option<f64> {
    (super::effects::name(e) == "Composite").then(|| {
        e["metadata"][EDITOR]["Parameters"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p["Parameter ID"] == "opacity")
            .and_then(|p| p["Parameter Value"].as_f64())
            .unwrap_or(100.0)
    })
}

pub fn with_opacity(t: &Title, effects: Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = effects
        .into_iter()
        .filter(|e| composite_opacity(e).is_none())
        .collect();
    if let Some(o) = t.opacity {
        out.push(json!({
            "OTIO_SCHEMA": "Effect.1",
            "metadata": { EDITOR: group(
                "Composite",
                1,
                1,
                vec![ranged(
                    keyed("opacity", json!(rounded(o * 100.0)), json!(100.0), "Double"),
                    0.0,
                    100.0,
                )],
            ) },
            "name": "",
            "effect_name": "Resolve Effect",
        }));
    }
    out
}

pub fn take_opacity(effects: &mut Vec<Value>) -> Option<f64> {
    let found = effects.iter().find_map(composite_opacity);
    effects.retain(|e| composite_opacity(e).is_none());
    found
        .map(|o| rounded((o / 100.0).clamp(0.0, 1.0)))
        .filter(|o| *o < 1.0)
}

fn kind_named(kind: &str) -> Option<(TitleKind, Option<TitleAlign>)> {
    match kind {
        "Rich" => Some((TitleKind::Text, None)),
        "Rich Lower Left" => Some((TitleKind::LowerThird, Some(TitleAlign::Left))),
        "Rich Lower Middle" => Some((TitleKind::LowerThird, Some(TitleAlign::Middle))),
        "Rich Lower Right" => Some((TitleKind::LowerThird, Some(TitleAlign::Right))),
        "Scroll" => Some((TitleKind::Scroll, None)),
        _ => None,
    }
}

fn scrolling(t: &Title, family: &str, points: f64, color: &str) -> Vec<Value> {
    let [x, _] = t.place();
    let (across, alignment) = match t.align() {
        TitleAlign::Left => (x + 0.5, 0),
        TitleAlign::Middle => (x, 1),
        TitleAlign::Right => (x - 0.5, 2),
    };
    vec![
        group(
            "Text",
            6,
            0,
            vec![
                plain(
                    "text",
                    json!(rich::plain(&t.text)),
                    json!("Scroll Title"),
                    "String",
                ),
                keyed(
                    "position",
                    json!([rounded(across), 0.5]),
                    json!([0.51, 0.5]),
                    "POINTF",
                ),
            ],
        ),
        group(
            "Formatting",
            7,
            0,
            vec![
                plain(
                    "font",
                    json!(format!("{family},-1,58,5,50,0,0,0,0,0")),
                    json!("Open Sans,-1,58,5,63,0,0,0,0,0"),
                    "String",
                ),
                ranged(
                    plain(
                        "size",
                        json!(points.round().clamp(24.0, 4096.0) as i64),
                        json!(216),
                        "Int",
                    ),
                    24.0,
                    4096.0,
                ),
                plain("alignment", json!(alignment), json!(0), "UInt"),
                plain("anchor", json!(4), json!(4), "UInt"),
                plain("color", json!(color), json!("#ffffff"), "Color"),
            ],
        ),
    ]
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn rounded(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

fn number(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

struct Base<'a> {
    family: &'a str,
    size: f64,
    height: f64,
    color: &'a str,
}

fn span_html(text: &str, look: &Look, base: &Base) -> String {
    let family = look.face.as_deref().unwrap_or(base.family);
    let points = look.size.unwrap_or(base.size) * base.height;
    let color = look
        .color
        .as_deref()
        .map_or_else(|| base.color.to_string(), str::to_string);
    let weight = if look.bold { 700 } else { 400 };
    let italic = if look.italic {
        " font-style:italic;"
    } else {
        ""
    };
    let under = if look.underline {
        " text-decoration: underline;"
    } else {
        ""
    };
    format!(
        "<span style=\" font-family:'{family}'; font-size:{}pt; font-weight:{weight};{italic}{under} \
         color:{color};\">{}</span>",
        number(points),
        escaped(text)
    )
}

fn html(lines: &[&[Span]], align: Option<&str>, spacing: u8, base: &Base) -> String {
    let align = align.map_or(String::new(), |a| format!(" align=\"{a}\""));
    let paragraphs: Vec<String> = lines
        .iter()
        .map(|line| {
            let spans: String = if line.is_empty() {
                span_html("", &Look::default(), base)
            } else {
                line.iter()
                    .map(|s| span_html(&s.text, &s.look, base))
                    .collect()
            };
            format!(
                "<p{align} style=\" margin-top:0px; margin-bottom:0px; margin-left:0px; \
                 margin-right:0px; -qt-block-indent:0; text-indent:0px; line-height:{spacing}; \
                 -qt-line-height-type: line-distance;\">{spans}</p>"
            )
        })
        .collect();
    format!("{HEAD}{}</body></html>", paragraphs.join("\n"))
}

fn plain_lines(lines: &[&[Span]]) -> String {
    lines
        .iter()
        .map(|l| l.iter().map(|s| s.text.as_str()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn plain(id: &str, value: Value, usual: Value, variant: &str) -> Value {
    json!({
        "Default Parameter Value": usual,
        "Parameter ID": id,
        "Parameter Value": value,
        "Variant Type": variant,
    })
}

fn keyed(id: &str, value: Value, usual: Value, variant: &str) -> Value {
    let mut p = plain(id, value, usual, variant);
    p["Key Frames"] = json!({});
    p
}

fn ranged(mut p: Value, low: f64, high: f64) -> Value {
    p["minValue"] = json!(low);
    p["maxValue"] = json!(high);
    p
}

fn group(name: &str, kind: u32, shown: u32, parameters: Vec<Value>) -> Value {
    json!({
        "Display Type": shown,
        "Effect Name": name,
        "Enabled": true,
        "Name": name,
        "Parameters": parameters,
        "Type": kind,
    })
}

fn rich_text(
    t: &Title,
    lines: &[&[Span]],
    blob: String,
    anchor: u32,
    at: [f64; 2],
    usual: [f64; 2],
) -> Value {
    let scale = rounded(t.scale());
    let zoom = |id: &str| ranged(keyed(id, json!(scale), json!(1.0), "Double"), 0.25, 4.0);
    group(
        "Rich Text",
        24,
        0,
        vec![
            plain(
                "rich text",
                json!(plain_lines(lines)),
                json!("Title"),
                "String",
            ),
            json!({ "Parameter ID": "title blob", "Title HTML": blob }),
            plain("anchor", json!(anchor), json!(anchor), "UInt"),
            keyed("position", json!(at), json!(usual), "POINTF"),
            zoom("transformationZoomX"),
            zoom("transformationZoomY"),
            plain("transformationZoomLink", json!(true), json!(true), "Bool"),
            ranged(
                keyed(
                    "transformationRotationAngle",
                    json!(rounded(t.turn())),
                    json!(0.0),
                    "Double",
                ),
                -100000.0,
                100000.0,
            ),
        ],
    )
}

#[derive(Debug, Clone, Default)]
pub struct Drawn {
    pub family: Option<String>,
    pub backing: Option<[f64; 4]>,
}

pub fn reference(t: &Title, height: u32, drawn: &Drawn) -> Value {
    let (kind, name) = kind_of(t);
    let family = t
        .style
        .font
        .as_deref()
        .or(drawn.family.as_deref())
        .unwrap_or(USUAL_FAMILY);
    let size = t.text_fraction();
    let points = size * f64::from(height);
    let color = hex(t.ink());
    let line = size * SubtitleStyle::LINE;
    let [x, y] = t.place();
    let spanned = rich::lines(&t.text);
    let lines: Vec<&[Span]> = spanned.iter().map(Vec::as_slice).collect();
    let base = Base {
        family,
        size,
        height: f64::from(height),
        color: &color,
    };
    let (mut groups, hold) = match t.kind {
        TitleKind::Scroll => (scrolling(t, family, points, &color), None),
        TitleKind::Text => (
            vec![rich_text(
                t,
                &lines,
                html(&lines, Some("center"), 0, &base),
                4,
                [rounded(x), rounded(1.0 - y)],
                [0.5, 0.5],
            )],
            Some([x, 1.0 - y]),
        ),
        TitleKind::LowerThird => {
            let (align, column, usual_x) = match t.align() {
                TitleAlign::Left => (None, 0, 0.1),
                TitleAlign::Middle => (Some("center"), 1, 0.5),
                TitleAlign::Right => (Some("right"), 2, 0.9),
            };
            let none: &[Span] = &[];
            let (upper, lower) = match lines.as_slice() {
                [] => (none, none),
                [only] => (none, *only),
                [first, second, ..] => (*first, *second),
            };
            let meet = 1.0 - y + t.lower_line();
            (
                vec![
                    rich_text(
                        t,
                        &[upper],
                        html(&[upper], align, 1, &base),
                        column,
                        [rounded(x), rounded(meet)],
                        [usual_x, 0.2],
                    ),
                    rich_text(
                        t,
                        &[lower],
                        html(&[lower], align, 0, &base),
                        6 + column,
                        [rounded(x), rounded(meet)],
                        [usual_x, 0.22],
                    ),
                ],
                Some([x, meet]),
            )
        }
    };
    let (shade, shift, blur, dim) = match &t.style.shadow {
        Some(s) => {
            let [dx, dy] = s.shift();
            (
                hex(s.ink()),
                [rounded(dx), rounded(-dy)],
                s.softness().round().clamp(1.0, 100.0) as i64,
                (s.alpha() * 100.0).round() as i64,
            )
        }
        None => ("#000000".into(), [0.0, 0.0], 20, 75),
    };
    groups.push(group(
        "Drop Shadow",
        8,
        1,
        vec![
            plain("shadow color", json!(shade), json!("#000000"), "Color"),
            keyed("shadow offset", json!(shift), json!([0.0, 0.0]), "POINTF"),
            ranged(keyed("shadow", json!(blur), json!(20), "Int"), 1.0, 100.0),
            ranged(
                keyed("shadow opacity", json!(dim), json!(75), "Int"),
                0.0,
                100.0,
            ),
        ],
    ));
    let (stroke_color, stroke) = match t.stroke() {
        Some((rgb, thickness)) => (
            hex(rgb),
            (thickness * STROKE_PER_TEXT).round().clamp(0.0, 16.0) as i64,
        ),
        None => (
            t.style
                .outline
                .as_ref()
                .and_then(|o| o.color.clone())
                .unwrap_or_else(|| "#000000".into()),
            0,
        ),
    };
    let usual_stroke = if t.kind == TitleKind::Text { 1 } else { 0 };
    let stroked = group(
        "Stroke",
        28,
        1,
        vec![
            plain(
                "strokeColor",
                json!(stroke_color),
                json!("#ffffff"),
                "Color",
            ),
            ranged(
                plain("strokeSize", json!(stroke), json!(usual_stroke), "Int"),
                0.0,
                16.0,
            ),
            plain("strokeOutsideOnly", json!(false), json!(false), "Bool"),
        ],
    );
    if t.kind != TitleKind::Scroll {
        groups.push(stroked);
    }
    let (back_color, opacity, extent, center, corner) = match t.backing() {
        Some((rgb, a)) => {
            let [left, top, w, h] = drawn.backing.unwrap_or_else(|| {
                let rows = lines.len().max(1) as f64;
                let h = rows * line + 2.0 * SubtitleStyle::PADDING * size;
                let top = match t.kind {
                    TitleKind::Text | TitleKind::Scroll => y - h / 2.0,
                    TitleKind::LowerThird => y + SubtitleStyle::PADDING * size - h,
                };
                let middle = if t.kind == TitleKind::Text { x } else { 0.5 };
                [middle - Title::SAFE / 2.0, top, Title::SAFE, h]
            });
            (
                hex(rgb),
                (a * 100.0).round() as i64,
                [rounded(w).min(2.0), rounded(h).min(2.0)],
                hold.map_or([0.0, 0.0], |[ax, ay]| {
                    [
                        rounded(left + w / 2.0 - ax),
                        rounded(1.0 - (top + h / 2.0) - ay),
                    ]
                }),
                0.0,
            )
        }
        None => ("#000000".into(), 50, [0.9, 0.0], [0.0, 0.0], 1.0 / 27.0),
    };
    groups.push(group(
        "Background",
        27,
        1,
        vec![
            plain(
                "backgroundColor",
                json!(back_color),
                json!("#000000"),
                "Color",
            ),
            plain(
                "backgroundOutlineColor",
                json!("#000000"),
                json!("#000000"),
                "Color",
            ),
            ranged(
                keyed("backgroundOutlineWidth", json!(0), json!(0), "Int"),
                0.0,
                30.0,
            ),
            ranged(
                keyed("backgroundWidth", json!(extent[0]), json!(0.9), "Double"),
                0.0,
                2.0,
            ),
            ranged(
                keyed("backgroundHeight", json!(extent[1]), json!(0.0), "Double"),
                0.0,
                2.0,
            ),
            ranged(
                keyed(
                    "backgroundCornerRadius",
                    json!(corner),
                    json!(1.0 / 27.0),
                    "Double",
                ),
                0.0,
                1.0,
            ),
            keyed(
                "backgroundCenter",
                json!(center),
                json!([0.0, 0.0]),
                "POINTF",
            ),
            ranged(
                keyed("backgroundOpacity", json!(opacity), json!(50), "Int"),
                0.0,
                100.0,
            ),
        ],
    ));
    json!({
        "OTIO_SCHEMA": "GeneratorReference.1",
        "metadata": { EDITOR: { "Generator Type": kind } },
        "name": name,
        "available_range": null,
        "available_image_bounds": null,
        "generator_kind": kind,
        "parameters": { EDITOR: groups },
    })
}

fn groups(reference: &Value) -> Vec<&Value> {
    reference["parameters"][EDITOR]
        .as_array()
        .into_iter()
        .flatten()
        .collect()
}

fn named<'a>(groups: &[&'a Value], name: &str) -> Vec<&'a Value> {
    groups
        .iter()
        .copied()
        .filter(|g| g["Effect Name"] == name)
        .collect()
}

fn value<'a>(group: &'a Value, id: &str) -> &'a Value {
    group["Parameters"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["Parameter ID"] == id)
        .map_or(&Value::Null, |p| {
            if p["Parameter ID"] == "title blob" {
                &p["Title HTML"]
            } else {
                &p["Parameter Value"]
            }
        })
}

#[derive(Debug, Clone, Default, PartialEq)]
struct Styled {
    family: Option<String>,
    points: Option<f64>,
    color: Option<String>,
    bold: bool,
    italic: bool,
    underline: bool,
}

#[derive(Debug, Default)]
struct Read {
    lines: Vec<Vec<(String, Styled)>>,
    family: Option<String>,
    points: Option<f64>,
    color: Option<String>,
}

fn styled(style: &str) -> Styled {
    Styled {
        family: style_value(style, "font-family").map(|f| f.trim_matches(['\'', '"']).to_string()),
        points: style_value(style, "font-size")
            .and_then(|s| s.strip_suffix("pt"))
            .and_then(|s| s.trim().parse().ok()),
        color: style_value(style, "color").map(str::to_string),
        bold: style_value(style, "font-weight")
            .and_then(|w| w.parse::<u32>().ok())
            .is_some_and(|w| w >= 600),
        italic: style_value(style, "font-style") == Some("italic"),
        underline: style_value(style, "text-decoration").is_some_and(|d| d.contains("underline")),
    }
}

fn markup(
    line: &[(String, Styled)],
    family: Option<&str>,
    points: Option<f64>,
    color: Option<&str>,
    height: f64,
) -> String {
    let spans: Vec<Span> = line
        .iter()
        .map(|(text, st)| Span {
            text: text.clone(),
            look: Look {
                bold: st.bold,
                italic: st.italic,
                underline: st.underline,
                color: st
                    .color
                    .as_deref()
                    .filter(|c| Some(*c) != color)
                    .and_then(crate::subtitles::color),
                face: st
                    .family
                    .as_deref()
                    .filter(|f| Some(*f) != family)
                    .filter(|f| crate::ops::subtitles::checked_font(Some(f), "title").is_ok())
                    .map(str::to_string),
                size: st
                    .points
                    .filter(|p| Some(*p) != points)
                    .map(|p| rounded(p / height))
                    .filter(|s| *s > 0.0 && *s <= 0.5),
            },
        })
        .collect();
    rich::write(&spans, &Look::default())
}

fn entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|&e| e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..end];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => name
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn style_value<'a>(style: &'a str, key: &str) -> Option<&'a str> {
    style.split(';').find_map(|part| {
        let (k, v) = part.split_once(':')?;
        (k.trim() == key).then(|| v.trim())
    })
}

fn read_html(blob: &str) -> Read {
    let mut read = Read::default();
    let body = blob.find("<body").map_or(blob, |i| &blob[i..]);
    let mut rest = body;
    while let Some(i) = rest.find("<p") {
        let after = &rest[i + 2..];
        if !after.starts_with([' ', '>']) {
            rest = after;
            continue;
        }
        let Some(open) = after.find('>') else {
            break;
        };
        let inner = &after[open + 1..];
        let close = inner.find("</p>").unwrap_or(inner.len());
        let paragraph = &inner[..close];
        let mut line: Vec<(String, Styled)> = Vec::new();
        let mut stack: Vec<Styled> = Vec::new();
        let mut cursor = paragraph;
        let put = |text: String, st: &Styled, line: &mut Vec<(String, Styled)>| {
            if text.is_empty() {
                return;
            }
            match line.last_mut() {
                Some((t, last)) if last == st => t.push_str(&text),
                _ => line.push((text, st.clone())),
            }
        };
        let mut lines: Vec<Vec<(String, Styled)>> = Vec::new();
        while let Some(t) = cursor.find('<') {
            let current = stack.last().cloned().unwrap_or_default();
            put(entities(&cursor[..t]), &current, &mut line);
            let Some(end) = cursor[t..].find('>') else {
                cursor = "";
                break;
            };
            let tag = &cursor[t + 1..t + end];
            if tag.starts_with("br") {
                lines.push(std::mem::take(&mut line));
            } else if tag.starts_with("/span") {
                stack.pop();
            } else if let Some(rest) = tag.strip_prefix("span") {
                let style = rest
                    .split_once("style=\"")
                    .and_then(|(_, s)| s.split_once('"'))
                    .map_or("", |(s, _)| s);
                let st = styled(style);
                if read.family.is_none() {
                    read.family.clone_from(&st.family);
                }
                if read.points.is_none() {
                    read.points = st.points;
                }
                if read.color.is_none() {
                    read.color.clone_from(&st.color);
                }
                stack.push(st);
            }
            cursor = &cursor[t + end + 1..];
        }
        let current = stack.last().cloned().unwrap_or_default();
        put(entities(cursor), &current, &mut line);
        lines.push(line);
        read.lines.extend(lines);
        rest = &inner[(close + 4).min(inner.len())..];
    }
    read
}

fn backing_of(all: &[&Value]) -> Option<SubtitleBackground> {
    named(all, "Background").first().and_then(|g| {
        (value(g, "backgroundHeight").as_f64().unwrap_or(0.0) > 0.0).then(|| SubtitleBackground {
            color: value(g, "backgroundColor")
                .as_str()
                .unwrap_or("#000000")
                .to_string(),
            opacity: value(g, "backgroundOpacity")
                .as_f64()
                .map(|o| (o / 100.0).clamp(0.0, 1.0)),
        })
    })
}

fn shadow_of(all: &[&Value]) -> Option<TitleShadow> {
    named(all, "Drop Shadow").first().and_then(|g| {
        let [dx, dy] = point(value(g, "shadow offset"))?;
        (dx != 0.0 || dy != 0.0).then(|| TitleShadow {
            color: value(g, "shadow color")
                .as_str()
                .filter(|c| crate::ops::subtitles::checked_color(Some(c), "shadow").is_ok())
                .map(str::to_string),
            opacity: value(g, "shadow opacity")
                .as_f64()
                .map(|o| (o / 100.0).clamp(0.0, 1.0)),
            offset: Some([
                rounded(dx.clamp(-0.5, 0.5)),
                rounded((-dy).clamp(-0.5, 0.5)),
            ]),
            blur: value(g, "shadow").as_f64().map(|b| b.clamp(0.0, 100.0)),
        })
    })
}

fn from_scroll(all: &[&Value], height: u32) -> Option<Title> {
    let text_group = *named(all, "Text").first()?;
    let looks = named(all, "Formatting").first().copied();
    let text = value(text_group, "text").as_str()?.replace("\r\n", "\n");
    let look = |id: &str| looks.map_or(&Value::Null, |g| value(g, id));
    let align = match look("alignment").as_u64() {
        Some(1) => TitleAlign::Middle,
        Some(2) => TitleAlign::Right,
        _ => TitleAlign::Left,
    };
    let across = point(value(text_group, "position")).map_or(0.51, |p| p[0]);
    let x = match align {
        TitleAlign::Left => across - 0.5,
        TitleAlign::Middle => across,
        TitleAlign::Right => across + 0.5,
    };
    let font = look("font")
        .as_str()
        .and_then(|f| f.split(',').next())
        .map(str::to_string)
        .filter(|f| crate::ops::subtitles::checked_font(Some(f), "title").is_ok());
    let size = look("size")
        .as_f64()
        .map(|p| p / f64::from(height.max(1)))
        .filter(|s| crate::ops::subtitles::checked_size(Some(*s), "title").is_ok());
    let color = look("color")
        .as_str()
        .map(str::to_string)
        .filter(|c| crate::ops::subtitles::checked_color(Some(c), "title").is_ok());
    let title = Title {
        kind: TitleKind::Scroll,
        align: Some(align),
        text,
        style: TitleStyle {
            font,
            size,
            color,
            outline: Some(SubtitleOutline {
                color: None,
                size: Some(0.0),
            }),
            background: backing_of(all),
            shadow: shadow_of(all),
        },
        position: Some([clamped(x), 0.5]),
        zoom: None,
        rotation: None,
        opacity: None,
    };
    crate::ops::titles::checked(&title).ok()
}

fn point(v: &Value) -> Option<[f64; 2]> {
    let a = v.as_array()?;
    Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?])
}

fn clamped(v: f64) -> f64 {
    rounded(v).clamp(0.0, 1.0)
}

pub fn read(item: &Value, reference: &Value, height: u32) -> Option<Title> {
    if let Some(raw) = item["metadata"][VIDCUT].get("title")
        && let Ok(t) = serde_json::from_value::<Title>(raw.clone())
        && let Ok(t) = crate::ops::titles::checked(&t)
    {
        return Some(t);
    }
    from_editor(reference, height)
}

pub fn from_editor(reference: &Value, height: u32) -> Option<Title> {
    let (kind, align) = kind_named(reference["generator_kind"].as_str()?)?;
    let all = groups(reference);
    if kind == TitleKind::Scroll {
        return from_scroll(&all, height);
    }
    let texts = named(&all, "Rich Text");
    let first = *texts.first()?;
    let read_of = |g: &Value| read_html(value(g, "title blob").as_str().unwrap_or(""));
    let row = |g: &Value| (value(g, "anchor").as_u64().unwrap_or(4) / 3).min(2);
    let h = f64::from(height.max(1));
    let marked = |r: &Read, base: &Read| -> Vec<String> {
        r.lines
            .iter()
            .map(|l| {
                markup(
                    l,
                    base.family.as_deref(),
                    base.points,
                    base.color.as_deref(),
                    h,
                )
            })
            .collect()
    };
    let (lines, style_from, at, lowered, turned_by) = match kind {
        TitleKind::Scroll => return None,
        TitleKind::Text => {
            let r = read_of(first);
            let at = point(value(first, "position")).unwrap_or([0.5, 0.5]);
            (marked(&r, &r), r, at, None, first)
        }
        TitleKind::LowerThird => {
            let (upper, lower) = match texts.get(1).copied() {
                Some(s) if row(first) == 2 && row(s) != 2 => (Some(s), first),
                Some(s) => (Some(first), s),
                None => (None, first),
            };
            let low = read_of(lower);
            let high = upper.map(read_of).unwrap_or_default();
            let style_from = if high.points.is_some() { high } else { low };
            let low = read_of(lower);
            let high = upper.map(read_of).unwrap_or_default();
            let text = |r: &Read| marked(r, &style_from).join(" ").trim().to_string();
            let lines: Vec<String> = [text(&high), text(&low)]
                .into_iter()
                .filter(|l| !l.is_empty())
                .collect();
            let at = point(value(lower, "position")).unwrap_or([0.1, 0.22]);
            (lines, style_from, at, Some(row(lower)), lower)
        }
    };
    let zoom = value(turned_by, "transformationZoomX")
        .as_f64()
        .filter(|z| z.is_finite())
        .map(|z| rounded(z.clamp(0.25, 4.0)));
    let rotation = value(turned_by, "transformationRotationAngle")
        .as_f64()
        .filter(|r| r.is_finite() && r.abs() <= 3600.0)
        .map(rounded);
    let size = style_from
        .points
        .map(|p| p / h)
        .filter(|s| crate::ops::subtitles::checked_size(Some(*s), "title").is_ok());
    let font = style_from
        .family
        .filter(|f| crate::ops::subtitles::checked_font(Some(f), "title").is_ok());
    let color = style_from
        .color
        .filter(|c| crate::ops::subtitles::checked_color(Some(c), "title").is_ok());
    let stroke = named(&all, "Stroke").first().copied();
    let outline = stroke.map(|g| {
        let steps = value(g, "strokeSize").as_f64().unwrap_or(0.0);
        if steps > 0.0 {
            SubtitleOutline {
                color: value(g, "strokeColor").as_str().map(str::to_string),
                size: Some((steps / STROKE_PER_TEXT).min(0.1)),
            }
        } else {
            SubtitleOutline {
                color: None,
                size: Some(0.0),
            }
        }
    });
    let background = backing_of(&all);
    let shadow = shadow_of(&all);
    let mut title = Title {
        kind,
        align,
        text: lines.join("\n"),
        style: TitleStyle {
            font,
            size,
            color,
            outline,
            background,
            shadow,
        },
        position: None,
        zoom,
        rotation,
        opacity: None,
    };
    let [x, from_bottom] = at;
    let mut y = 1.0 - from_bottom;
    if let Some(row) = lowered {
        y += row as f64 / 2.0 * title.lower_line();
    }
    title.position = Some([clamped(x), clamped(y)]);
    crate::ops::titles::checked(&title).ok()
}
