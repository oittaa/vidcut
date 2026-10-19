use serde_json::{Value, json};

use super::{EDITOR, VIDCUT};
use crate::model::{
    SubtitleBackground, SubtitleOutline, SubtitleStyle, Title, TitleAlign, TitleKind, TitleShadow,
    TitleStyle,
};

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
    }
}

pub fn name(t: &Title) -> &'static str {
    kind_of(t).1
}

fn kind_named(kind: &str) -> Option<(TitleKind, Option<TitleAlign>)> {
    match kind {
        "Rich" => Some((TitleKind::Text, None)),
        "Rich Lower Left" => Some((TitleKind::LowerThird, Some(TitleAlign::Left))),
        "Rich Lower Middle" => Some((TitleKind::LowerThird, Some(TitleAlign::Middle))),
        "Rich Lower Right" => Some((TitleKind::LowerThird, Some(TitleAlign::Right))),
        _ => None,
    }
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

fn html(
    lines: &[&str],
    align: Option<&str>,
    spacing: u8,
    family: &str,
    points: f64,
    color: &str,
) -> String {
    let align = align.map_or(String::new(), |a| format!(" align=\"{a}\""));
    let paragraphs: Vec<String> = lines
        .iter()
        .map(|line| {
            format!(
                "<p{align} style=\" margin-top:0px; margin-bottom:0px; margin-left:0px; \
                 margin-right:0px; -qt-block-indent:0; text-indent:0px; line-height:{spacing}; \
                 -qt-line-height-type: line-distance;\"><span style=\" font-family:'{family}'; \
                 font-size:{}pt; font-weight:400; color:{color};\">{}</span></p>",
                number(points),
                escaped(line)
            )
        })
        .collect();
    format!("{HEAD}{}</body></html>", paragraphs.join("\n"))
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
    lines: &[&str],
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
                json!(lines.join("\n")),
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
    let lines: Vec<&str> = t.text.lines().collect();
    let (mut groups, hold) = match t.kind {
        TitleKind::Text => (
            vec![rich_text(
                t,
                &lines,
                html(&lines, Some("center"), 0, family, points, &color),
                4,
                [rounded(x), rounded(1.0 - y)],
                [0.5, 0.5],
            )],
            [x, 1.0 - y],
        ),
        TitleKind::LowerThird => {
            let (align, column, usual_x) = match t.align() {
                TitleAlign::Left => (None, 0, 0.1),
                TitleAlign::Middle => (Some("center"), 1, 0.5),
                TitleAlign::Right => (Some("right"), 2, 0.9),
            };
            let (upper, lower) = match lines.as_slice() {
                [] => ("", ""),
                [only] => ("", *only),
                [first, second, ..] => (*first, *second),
            };
            let meet = 1.0 - y + line;
            (
                vec![
                    rich_text(
                        t,
                        &[upper],
                        html(&[upper], align, 1, family, points, &color),
                        column,
                        [rounded(x), rounded(meet)],
                        [usual_x, 0.2],
                    ),
                    rich_text(
                        t,
                        &[lower],
                        html(&[lower], align, 0, family, points, &color),
                        6 + column,
                        [rounded(x), rounded(meet)],
                        [usual_x, 0.22],
                    ),
                ],
                [x, meet],
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
    groups.push(group(
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
    ));
    let (back_color, opacity, extent, center, corner) = match t.backing() {
        Some((rgb, a)) => {
            let [left, top, w, h] = drawn.backing.unwrap_or_else(|| {
                let rows = lines.len().max(1) as f64;
                let h = rows * line + 2.0 * SubtitleStyle::PADDING * size;
                let top = match t.kind {
                    TitleKind::Text => y - h / 2.0,
                    TitleKind::LowerThird => y + SubtitleStyle::PADDING * size - h,
                };
                let middle = if t.kind == TitleKind::Text { x } else { 0.5 };
                [middle - Title::SAFE / 2.0, top, Title::SAFE, h]
            });
            (
                hex(rgb),
                (a * 100.0).round() as i64,
                [rounded(w).min(2.0), rounded(h).min(2.0)],
                [
                    rounded(left + w / 2.0 - hold[0]),
                    rounded(1.0 - (top + h / 2.0) - hold[1]),
                ],
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

#[derive(Debug, Default)]
struct Read {
    lines: Vec<String>,
    family: Option<String>,
    points: Option<f64>,
    color: Option<String>,
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
        let mut text = String::new();
        let mut cursor = paragraph;
        while let Some(t) = cursor.find('<') {
            text.push_str(&entities(&cursor[..t]));
            let Some(end) = cursor[t..].find('>') else {
                cursor = "";
                break;
            };
            let tag = &cursor[t + 1..t + end];
            if tag.starts_with("br") {
                text.push('\n');
            }
            if let Some(style) = tag
                .strip_prefix("span")
                .and_then(|s| s.split_once("style=\""))
                .and_then(|(_, s)| s.split_once('"'))
                .map(|(s, _)| s)
            {
                if read.family.is_none() {
                    read.family = style_value(style, "font-family")
                        .map(|f| f.trim_matches(['\'', '"']).to_string());
                }
                if read.points.is_none() {
                    read.points = style_value(style, "font-size")
                        .and_then(|s| s.strip_suffix("pt"))
                        .and_then(|s| s.trim().parse().ok());
                }
                if read.color.is_none() {
                    read.color = style_value(style, "color").map(str::to_string);
                }
            }
            cursor = &cursor[t + end + 1..];
        }
        text.push_str(&entities(cursor));
        read.lines.extend(text.split('\n').map(str::to_string));
        rest = &inner[(close + 4).min(inner.len())..];
    }
    read
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
    let texts = named(&all, "Rich Text");
    let first = *texts.first()?;
    let read_of = |g: &Value| read_html(value(g, "title blob").as_str().unwrap_or(""));
    let row = |g: &Value| (value(g, "anchor").as_u64().unwrap_or(4) / 3).min(2);
    let (lines, style_from, at, lowered, turned_by) = match kind {
        TitleKind::Text => {
            let r = read_of(first);
            let at = point(value(first, "position")).unwrap_or([0.5, 0.5]);
            (r.lines.clone(), r, at, None, first)
        }
        TitleKind::LowerThird => {
            let (upper, lower) = match texts.get(1).copied() {
                Some(s) if row(first) == 2 && row(s) != 2 => (Some(s), first),
                Some(s) => (Some(first), s),
                None => (None, first),
            };
            let low = read_of(lower);
            let high = upper.map(read_of).unwrap_or_default();
            let text = |r: &Read| r.lines.join(" ").trim().to_string();
            let lines: Vec<String> = [text(&high), text(&low)]
                .into_iter()
                .filter(|l| !l.is_empty())
                .collect();
            let at = point(value(lower, "position")).unwrap_or([0.1, 0.22]);
            let style_from = if high.points.is_some() { high } else { low };
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
        .map(|p| p / f64::from(height.max(1)))
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
    let background = named(&all, "Background").first().and_then(|g| {
        (value(g, "backgroundHeight").as_f64().unwrap_or(0.0) > 0.0).then(|| SubtitleBackground {
            color: value(g, "backgroundColor")
                .as_str()
                .unwrap_or("#000000")
                .to_string(),
            opacity: value(g, "backgroundOpacity")
                .as_f64()
                .map(|o| (o / 100.0).clamp(0.0, 1.0)),
        })
    });
    let shadow = named(&all, "Drop Shadow").first().and_then(|g| {
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
    });
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
    };
    let [x, from_bottom] = at;
    let mut y = 1.0 - from_bottom;
    if let Some(row) = lowered {
        y += row as f64 / 2.0 * title.text_fraction() * SubtitleStyle::LINE;
    }
    title.position = Some([clamped(x), clamped(y)]);
    crate::ops::titles::checked(&title).ok()
}
