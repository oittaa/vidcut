use serde_json::{Value, json};

use super::effects::{effect, group, handle_point, handle_value, keyed, plain, ranged, rounded};
use super::picture::{file_curve, read_keys};
use super::{EDITOR, VIDCUT};
use crate::model::hex;
use crate::model::{
    Keyframe, Picture, PositionHandles, SubtitleBackground, SubtitleOutline, TextStroke, Title,
    TitleAlign, TitleAnchor, TitleCase, TitleKind, TitleLine, TitlePart, TitleShadow, TitleShape,
    TitleStyle, size_fits,
};
use crate::rich::{self, Look, Shift, Span};

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

fn parameter<'a>(group: &'a Value, id: &str) -> &'a Value {
    group["Parameters"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["Parameter ID"] == id)
        .unwrap_or(&Value::Null)
}

fn composite(e: &Value) -> Option<&Value> {
    (super::effects::name(e) == "Composite").then(|| parameter(&e["metadata"][EDITOR], "opacity"))
}

fn with_frames(mut p: Value, frames: Option<Value>) -> Value {
    if let Some(frames) = frames {
        p["Key Frames"] = frames;
    }
    p
}

fn point_frames(t: &Title, origin: i64, file: impl Fn([f64; 2]) -> [f64; 2]) -> Option<Value> {
    if t.position_keys.is_empty() {
        return None;
    }
    let mut out = serde_json::Map::new();
    for (k, y) in t.position_keys.iter().zip(&t.position_y) {
        let [x, y] = file([k.value, *y]);
        let mut key = json!({ "Value": [rounded(x), rounded(y)], "Variant Type": "POINTF" });
        if let Some(h) = t.position_handles.iter().find(|h| h.frame == k.frame) {
            let [before, after] = ["InBez", "OutBez"];
            if let Some([x, y]) = h.in_handle {
                key[before] = handle_value(before, x, 0.0 - y);
            }
            if let Some([x, y]) = h.out_handle {
                key[after] = handle_value(after, x, 0.0 - y);
            }
        }
        out.insert((k.frame - origin).to_string(), key);
    }
    Some(Value::Object(out))
}

fn fitted(keys: &[Keyframe], by: f64, low: f64, high: f64) -> Vec<Keyframe> {
    let scaled = |h: crate::model::Handle| crate::model::Handle {
        frames: h.frames,
        value: rounded(h.value * by),
    };
    keys.iter()
        .map(|k| Keyframe {
            value: rounded((k.value * by).clamp(low, high)),
            in_handle: k.in_handle.map(scaled),
            out_handle: k.out_handle.map(scaled),
            frame: k.frame,
        })
        .collect()
}

fn point_keys(
    p: &Value,
    origin: i64,
    ours: impl Fn([f64; 2]) -> [f64; 2],
) -> (Vec<Keyframe>, Vec<f64>) {
    let keys = p.get("Key Frames").and_then(Value::as_object);
    let mut rows: Vec<(Keyframe, f64)> = Vec::new();
    for (t, k) in keys.into_iter().flatten() {
        if let (Ok(at), Some(v)) = (t.parse::<f64>(), point(&k["Value"])) {
            let [x, y] = ours(v);
            let key = Keyframe {
                frame: (at + origin as f64).round() as i64,
                value: across(x),
                in_handle: None,
                out_handle: None,
            };
            rows.push((key, down(y)));
        }
    }
    rows.sort_by_key(|(k, _)| k.frame);
    rows.dedup_by_key(|(k, _)| k.frame);
    rows.into_iter().unzip()
}

fn point_handles(p: &Value, origin: i64) -> Vec<PositionHandles> {
    let handle = |k: &Value, name: &str| {
        let h = handle_point(k, name)?;
        Some([h.first()?.as_f64()?, -h.get(1)?.as_f64()?])
    };
    let keys = p.get("Key Frames").and_then(Value::as_object);
    let mut out: Vec<PositionHandles> = keys
        .into_iter()
        .flatten()
        .filter_map(|(t, k)| {
            let (in_handle, out_handle) = (handle(k, "InBez"), handle(k, "OutBez"));
            (in_handle.is_some() || out_handle.is_some()).then_some(())?;
            Some(PositionHandles {
                frame: (t.parse::<f64>().ok()? + origin as f64).round() as i64,
                in_handle,
                out_handle,
            })
        })
        .collect();
    out.sort_by_key(|h| h.frame);
    out.dedup_by_key(|h| h.frame);
    out
}

pub fn with_opacity(t: &Title, effects: Vec<Value>, origin: i64) -> Vec<Value> {
    let mut out: Vec<Value> = effects
        .into_iter()
        .filter(|e| composite(e).is_none())
        .collect();
    if t.opacity.is_some() || !t.opacity_keys.is_empty() {
        out.push(effect(group(
            "Composite",
            1,
            1,
            vec![ranged(
                with_frames(
                    keyed(
                        "opacity",
                        json!(rounded(t.opaque() * 100.0)),
                        json!(100.0),
                        "Double",
                    ),
                    file_curve(&t.opacity_keys, None, origin, |o| rounded(o * 100.0), 100.0),
                ),
                0.0,
                100.0,
            )],
        )));
    }
    out
}

pub fn take_opacity(effects: &mut Vec<Value>, origin: i64) -> (Option<f64>, Vec<Keyframe>) {
    let found = effects.iter().find_map(composite).cloned();
    effects.retain(|e| composite(e).is_none());
    let Some(p) = found else {
        return (None, Vec::new());
    };
    let plain = p["Parameter Value"].as_f64().unwrap_or(100.0);
    let keys = read_keys(&p, origin).unwrap_or_default();
    (
        Some(rounded((plain / 100.0).clamp(0.0, 1.0))).filter(|o| *o < 1.0),
        fitted(&keys, 0.01, 0.0, 1.0),
    )
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

fn scrolling(t: &Title, family: &str, points: f64, color: &str, origin: i64) -> Vec<Value> {
    let [x, y] = t.place();
    let (shift, alignment) = match t.align() {
        TitleAlign::Left => (0.5, 0),
        TitleAlign::Middle => (0.0, 1),
        TitleAlign::Right => (-0.5, 2),
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
                with_frames(
                    keyed(
                        "position",
                        json!([rounded(x + shift), rounded(1.0 - y)]),
                        json!([0.51, 0.5]),
                        "POINTF",
                    ),
                    point_frames(t, origin, |[x, y]| [x + shift, 1.0 - y]),
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
                plain(
                    "anchor",
                    json!(anchor_number(t.anchor.unwrap_or_default())),
                    json!(anchor_number(TitleAnchor::default())),
                    "UInt",
                ),
                plain("color", json!(color), json!("#ffffff"), "Color"),
            ],
        ),
    ]
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
    tracking: f64,
    case: TitleCase,
}

fn span_html(text: &str, look: &Look, before: &str, base: &Base) -> String {
    let family = look.face.as_deref().unwrap_or(base.family);
    let points = look.size.unwrap_or(base.size) * base.height;
    let color = look
        .color
        .as_deref()
        .map_or_else(|| base.color.to_string(), str::to_string);
    let weight = if look.bold { 600 } else { 400 };
    let italic = if look.italic {
        " font-style:italic;"
    } else {
        ""
    };
    let bars: Vec<&str> = [
        (look.underline, "underline"),
        (look.overline, "overline"),
        (look.strike, "line-through"),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect();
    let under = if bars.is_empty() {
        String::new()
    } else {
        format!(" text-decoration: {};", bars.join(" "))
    };
    let shifted = match look.shift {
        Some(Shift::Up) => " vertical-align:super;",
        Some(Shift::Down) => " vertical-align:sub;",
        None => "",
    };
    let case = look.case.unwrap_or(base.case);
    let cased = match case {
        TitleCase::Mixed => "",
        TitleCase::Upper => " text-transform:uppercase;",
        TitleCase::Lower => " text-transform:lowercase;",
        TitleCase::Title => " text-transform:capitalize;",
        TitleCase::Small => " font-variant:small-caps;",
    };
    let letters: String = match case {
        TitleCase::Title => case
            .pieces(text, before)
            .into_iter()
            .map(|(t, _)| t)
            .collect(),
        _ => text.to_string(),
    };
    let tracked = match look.tracking.unwrap_or(base.tracking) {
        0.0 => String::new(),
        t => format!(" letter-spacing:{}px;", number(Title::tracking_to_file(t))),
    };
    format!(
        "<span style=\" font-family:'{family}'; font-size:{}pt; font-weight:{weight};{italic}{under}{shifted}{tracked}{cased} \
         color:{color};\">{}</span>",
        number(points),
        escaped(&letters)
    )
}

fn html(lines: &[&[Span]], align: Option<&str>, spacing: f64, base: &Base) -> String {
    let align = align.map_or(String::new(), |a| format!(" align=\"{a}\""));
    let paragraphs: Vec<String> = lines
        .iter()
        .map(|line| {
            let spans: String = if line.is_empty() {
                span_html("", &Look::default(), "", base)
            } else {
                let mut before = String::new();
                line.iter()
                    .map(|s| {
                        let span = span_html(&s.text, &s.look, &before, base);
                        before.push_str(&s.text);
                        span
                    })
                    .collect()
            };
            let spacing = number(rich::spacing(line).unwrap_or(spacing));
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

const ANCHORS: [TitleAnchor; 9] = [
    TitleAnchor::BottomLeft,
    TitleAnchor::Bottom,
    TitleAnchor::BottomRight,
    TitleAnchor::Left,
    TitleAnchor::Middle,
    TitleAnchor::Right,
    TitleAnchor::TopLeft,
    TitleAnchor::Top,
    TitleAnchor::TopRight,
];

fn anchor_number(anchor: TitleAnchor) -> u32 {
    ANCHORS
        .iter()
        .position(|a| *a == anchor)
        .map_or(4, |i| i as u32)
}

fn anchor_read(g: &Value, usual: TitleAnchor) -> Option<TitleAnchor> {
    let number = usize::try_from(value(g, "anchor").as_u64()?).ok()?;
    ANCHORS.get(number).copied().filter(|a| *a != usual)
}

fn placed(t: &Title, usual: [f64; 2], origin: i64) -> Value {
    let [x, y] = t.in_file(t.place());
    with_frames(
        keyed(
            "position",
            json!([rounded(x), rounded(y)]),
            json!(usual),
            "POINTF",
        ),
        point_frames(t, origin, |at| t.in_file(at)),
    )
}

fn rich_text(
    t: &Title,
    lines: &[&[Span]],
    blob: String,
    (anchor, usual): (TitleAnchor, TitleAnchor),
    position: Value,
    origin: i64,
) -> Value {
    let scale = rounded(t.scale());
    let zoom = |id: &str| {
        ranged(
            with_frames(
                keyed(id, json!(scale), json!(1.0), "Double"),
                file_curve(&t.zoom_keys, None, origin, rounded, 1.0),
            ),
            0.25,
            4.0,
        )
    };
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
            plain(
                "anchor",
                json!(anchor_number(anchor)),
                json!(anchor_number(usual)),
                "UInt",
            ),
            position,
            zoom("transformationZoomX"),
            zoom("transformationZoomY"),
            plain("transformationZoomLink", json!(true), json!(true), "Bool"),
            ranged(
                with_frames(
                    keyed(
                        "transformationRotationAngle",
                        json!(rounded(t.turn())),
                        json!(0.0),
                        "Double",
                    ),
                    file_curve(&t.rotation_keys, None, origin, rounded, 1.0),
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

pub fn reference(t: &Title, height: u32, drawn: &Drawn, origin: i64) -> Value {
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
    let line = size * Title::LINE;
    let [x, y] = t.place();
    let spanned = rich::lines(&t.sized_text());
    let lines: Vec<&[Span]> = spanned.iter().map(Vec::as_slice).collect();
    let base = Base {
        family,
        size,
        height: f64::from(height),
        color: &color,
        tracking: t.style.tracking.unwrap_or(0.0),
        case: t.style.case.unwrap_or_default(),
    };
    let align = match t.align() {
        TitleAlign::Left => None,
        TitleAlign::Middle => Some("center"),
        TitleAlign::Right => Some("right"),
    };
    let (mut groups, hold) = match t.kind {
        TitleKind::Scroll => (scrolling(t, family, points, &color, origin), None),
        TitleKind::Text => (
            vec![rich_text(
                t,
                &lines,
                html(&lines, align, t.style.line_spacing.unwrap_or(0.0), &base),
                (t.anchor.unwrap_or_default(), TitleAnchor::default()),
                placed(t, [0.5, 0.5], origin),
                origin,
            )],
            Some(t.in_file([x, y])),
        ),
        TitleKind::LowerThird => {
            let usual_x = Title::usual_position(t.kind, t.align())[0];
            let usual = |part| Title::usual_anchor(t.kind, t.align(), part);
            let none: &[Span] = &[];
            let (upper, lower) = match lines.as_slice() {
                [] => (none, none),
                [only] => (none, *only),
                [first, second, ..] => (*first, *second),
            };
            let high = t.upper_view();
            (
                vec![
                    rich_text(
                        &high,
                        &[upper],
                        html(&[upper], align, t.style.line_spacing.unwrap_or(1.0), &base),
                        (
                            high.anchor.unwrap_or(usual(TitlePart::Upper)),
                            usual(TitlePart::Upper),
                        ),
                        placed(&high, [usual_x, 0.2], origin),
                        origin,
                    ),
                    rich_text(
                        t,
                        &[lower],
                        html(&[lower], align, t.style.line_spacing.unwrap_or(0.0), &base),
                        (
                            t.anchor.unwrap_or(usual(TitlePart::Lower)),
                            usual(TitlePart::Lower),
                        ),
                        placed(t, [usual_x, 0.22], origin),
                        origin,
                    ),
                ],
                Some(t.in_file([x, y])),
            )
        }
    };
    let (shade, shift, blur, dim) = match t.style.shadow.as_ref().filter(|s| !s.is_off()) {
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
    let (stroke_color, stroke, outside) = match t.stroke() {
        Some(s) => (
            hex(s.ink),
            s.width.round().clamp(0.0, TextStroke::MOST) as i64,
            s.outside,
        ),
        None => (
            t.style
                .outline
                .as_ref()
                .and_then(|o| o.color.clone())
                .unwrap_or_else(|| "#000000".into()),
            0,
            false,
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
            plain("strokeOutsideOnly", json!(outside), json!(false), "Bool"),
        ],
    );
    if t.kind != TitleKind::Scroll {
        groups.push(stroked);
    }
    let (back_color, opacity, extent, center, corner, ring, ring_width) = match t.backing() {
        Some((rgb, a)) if t.shape().is_some() => {
            let s = t.shape().expect("just checked");
            let [across, down] = s.center.unwrap_or([0.0, 0.0]);
            let (ring, width) = t.shape_outline();
            (
                hex(rgb),
                (a * 100.0).round() as i64,
                s.size,
                [across, -down],
                s.corner.unwrap_or(Title::BOX_CORNER),
                hex(ring),
                width.round() as i64,
            )
        }
        Some((rgb, a)) => {
            let [left, top, w, h] = drawn.backing.unwrap_or_else(|| {
                let rows = lines.len().max(1) as f64;
                let h = rows * line + 2.0 * Title::PADDING * size;
                let [across, down] = t.anchor_of(TitlePart::Lower);
                let top = match t.kind {
                    TitleKind::Text => y - h * down,
                    TitleKind::Scroll => y - h / 2.0,
                    TitleKind::LowerThird => {
                        y + (1.0 - down) * t.lower_line() + Title::PADDING * size - h
                    }
                };
                let middle = if t.kind == TitleKind::Text {
                    x + (0.5 - across) * Title::SAFE
                } else {
                    0.5
                };
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
                "#000000".to_string(),
                0,
            )
        }
        None => (
            "#000000".into(),
            50,
            [0.9, 0.0],
            [0.0, 0.0],
            Title::BOX_CORNER,
            "#000000".to_string(),
            0,
        ),
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
                json!(ring),
                json!("#000000"),
                "Color",
            ),
            ranged(
                keyed("backgroundOutlineWidth", json!(ring_width), json!(0), "Int"),
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
                    json!(Title::BOX_CORNER),
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
    let p = parameter(group, id);
    if p["Parameter ID"] == "title blob" {
        &p["Title HTML"]
    } else {
        &p["Parameter Value"]
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
struct Styled {
    family: Option<String>,
    points: Option<f64>,
    color: Option<String>,
    bold: bool,
    italic: bool,
    underline: bool,
    overline: bool,
    strike: bool,
    shift: Option<Shift>,
    tracking: f64,
    case: TitleCase,
}

#[derive(Debug, Default)]
struct Read {
    lines: Vec<Vec<(String, Styled)>>,
    spacings: Vec<Option<f64>>,
    align: Option<TitleAlign>,
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
        overline: style_value(style, "text-decoration").is_some_and(|d| d.contains("overline")),
        strike: style_value(style, "text-decoration").is_some_and(|d| d.contains("line-through")),
        shift: match style_value(style, "vertical-align") {
            Some("super") => Some(Shift::Up),
            Some("sub") => Some(Shift::Down),
            _ => None,
        },
        tracking: style_value(style, "letter-spacing")
            .and_then(|s| s.strip_suffix("px"))
            .and_then(|s| s.trim().parse().ok())
            .map(Title::tracking_from_file)
            .filter(|v| Title::spacing_fits(*v))
            .unwrap_or(0.0),
        case: if style_value(style, "font-variant") == Some("small-caps") {
            TitleCase::Small
        } else {
            match style_value(style, "text-transform") {
                Some("uppercase") => TitleCase::Upper,
                Some("lowercase") => TitleCase::Lower,
                Some("capitalize") => TitleCase::Title,
                _ => TitleCase::Mixed,
            }
        },
    }
}

fn markup(
    spacing: Option<f64>,
    tracking: f64,
    case: TitleCase,
    line: &[(String, Styled)],
    base: &Read,
    height: f64,
) -> String {
    let family = base.family.as_deref();
    let points = base.points;
    let color = base.color.as_deref();
    let spans: Vec<Span> = line
        .iter()
        .map(|(text, st)| Span {
            text: text.clone(),
            look: Look {
                bold: st.bold,
                italic: st.italic,
                underline: st.underline,
                overline: st.overline,
                strike: st.strike,
                shift: st.shift,
                spacing,
                tracking: (st.tracking != tracking).then_some(st.tracking),
                case: (st.case != case).then_some(st.case),
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
                    .filter(|s| size_fits(*s, Title::MOST_SIZE)),
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
        let spacing = after[..open]
            .split_once("style=\"")
            .and_then(|(_, s)| s.split_once('"'))
            .and_then(|(s, _)| style_value(s, "line-height"))
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| Title::spacing_fits(*v));
        read.align
            .get_or_insert(if after[..open].contains("align=\"center\"") {
                TitleAlign::Middle
            } else if after[..open].contains("align=\"right\"") {
                TitleAlign::Right
            } else {
                TitleAlign::Left
            });
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
        read.spacings
            .extend(std::iter::repeat_n(spacing, lines.len()));
        read.lines.extend(lines);
        rest = &inner[(close + 4).min(inner.len())..];
    }
    read
}

fn box_size(g: &Value) -> Option<[f64; 2]> {
    let side = |name| value(g, name).as_f64().unwrap_or(0.0);
    let size = [side("backgroundWidth"), side("backgroundHeight")];
    (size[0] > 0.0 && size[1] > 0.0).then(|| size.map(|v| rounded(v.min(Title::BOX_MOST))))
}

fn shape_of(all: &[&Value]) -> Option<TitleShape> {
    let groups = named(all, "Background");
    let g = groups.first()?;
    let [across, up] = point(value(g, "backgroundCenter")).unwrap_or([0.0, 0.0]);
    let width = value(g, "backgroundOutlineWidth")
        .as_f64()
        .unwrap_or(0.0)
        .clamp(0.0, Title::BOX_MOST_OUTLINE);
    Some(TitleShape {
        size: box_size(g)?,
        center: (across != 0.0 || up != 0.0)
            .then(|| [across, -up].map(|v| rounded(v.clamp(-Picture::REACH, Picture::REACH)))),
        corner: value(g, "backgroundCornerRadius")
            .as_f64()
            .map(|c| c.clamp(0.0, 1.0)),
        outline_color: (width > 0.0)
            .then(|| value(g, "backgroundOutlineColor").as_str())
            .flatten()
            .filter(|c| crate::ops::subtitles::checked_color(Some(c), "outline").is_ok())
            .map(str::to_string),
        outline_width: (width > 0.0).then_some(width),
    })
}

fn backing_of(all: &[&Value]) -> Option<SubtitleBackground> {
    named(all, "Background").first().and_then(|g| {
        box_size(g).map(|_| SubtitleBackground {
            color: value(g, "backgroundColor")
                .as_str()
                .unwrap_or("#000000")
                .to_string(),
            opacity: value(g, "backgroundOpacity")
                .as_f64()
                .map(|o| (o / 100.0).clamp(0.0, 1.0)),
            off: None,
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
                rounded(dx.clamp(-Picture::REACH, Picture::REACH)),
                rounded((-dy).clamp(-Picture::REACH, Picture::REACH)),
            ]),
            blur: value(g, "shadow").as_f64().map(|b| b.clamp(1.0, 100.0)),
            off: None,
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
    let [middle, up] = point(value(text_group, "position")).unwrap_or([0.51, 0.5]);
    let x = match align {
        TitleAlign::Left => middle - 0.5,
        TitleAlign::Middle => middle,
        TitleAlign::Right => middle + 0.5,
    };
    let font = look("font")
        .as_str()
        .and_then(|f| f.split(',').next())
        .map(str::to_string)
        .filter(|f| crate::ops::subtitles::checked_font(Some(f), "title").is_ok());
    let size = look("size")
        .as_f64()
        .map(|p| p / f64::from(height.max(1)))
        .filter(|s| size_fits(*s, Title::MOST_SIZE));
    let color = look("color")
        .as_str()
        .map(str::to_string)
        .filter(|c| crate::ops::subtitles::checked_color(Some(c), "title").is_ok());
    let title = Title {
        kind: TitleKind::Scroll,
        align: Some(align),
        anchor: looks.and_then(|g| anchor_read(g, TitleAnchor::default())),
        text,
        style: TitleStyle {
            font,
            size,
            line_spacing: None,
            tracking: None,
            case: None,
            color,
            outline: Some(SubtitleOutline {
                color: None,
                size: Some(0.0),
                outside: None,
            }),
            background: backing_of(all),
            shape: shape_of(all),
            shadow: shadow_of(all),
        },
        position: Some([across(x), down(1.0 - up)]),
        position_keys: Vec::new(),
        position_y: Vec::new(),
        position_handles: Vec::new(),
        zoom: None,
        zoom_keys: Vec::new(),
        rotation: None,
        rotation_keys: Vec::new(),
        opacity: None,
        opacity_keys: Vec::new(),
        upper: None,
    };
    crate::ops::titles::checked(&title).ok()
}

fn point(v: &Value) -> Option<[f64; 2]> {
    let a = v.as_array()?;
    Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?])
}

fn across(v: f64) -> f64 {
    rounded(v).clamp(Title::ACROSS.0, Title::ACROSS.1)
}

fn down(v: f64) -> f64 {
    rounded(v).clamp(Title::DOWN.0, Title::DOWN.1)
}

fn upper_lower<'a>(texts: &[&'a Value]) -> Option<(Option<&'a Value>, &'a Value)> {
    match texts {
        [] => None,
        [only] => Some((None, *only)),
        [upper, lower, ..] => Some((Some(*upper), *lower)),
    }
}

fn read_scroll_keys(t: &mut Title, all: &[&Value], origin: i64) {
    let Some(text) = named(all, "Text").first().copied() else {
        return;
    };
    let shift = match t.align() {
        TitleAlign::Left => 0.5,
        TitleAlign::Middle => 0.0,
        TitleAlign::Right => -0.5,
    };
    (t.position_keys, t.position_y) = point_keys(parameter(text, "position"), origin, |[x, y]| {
        [x - shift, 1.0 - y]
    });
}

fn read_rich_keys(t: &mut Title, all: &[&Value], origin: i64) {
    let texts = named(all, "Rich Text");
    let Some((_, lower)) = upper_lower(&texts) else {
        return;
    };
    let main = if t.kind == TitleKind::Text {
        texts[0]
    } else {
        lower
    };
    (t.position_keys, t.position_y) =
        point_keys(parameter(main, "position"), origin, |[x, y]| [x, 1.0 - y]);
    t.position_handles = point_handles(parameter(main, "position"), origin);
    read_turn_keys(t, main, origin);
    if t.kind == TitleKind::LowerThird
        && t.text.contains('\n')
        && let Some((Some(high), _)) = upper_lower(&texts)
    {
        let mut line = t.upper_view();
        (line.position_keys, line.position_y) =
            point_keys(parameter(high, "position"), origin, |[x, y]| [x, 1.0 - y]);
        line.position_handles = point_handles(parameter(high, "position"), origin);
        read_turn_keys(&mut line, high, origin);
        t.upper = Some(line.line()).filter(|u| *u != t.upper_with_lower());
    }
}

fn read_turn_keys(t: &mut Title, g: &Value, origin: i64) {
    let keys = |id: &str| read_keys(parameter(g, id), origin).unwrap_or_default();
    t.zoom_keys = fitted(&keys("transformationZoomX"), 1.0, 0.25, 4.0);
    t.rotation_keys = fitted(
        &keys("transformationRotationAngle"),
        1.0,
        -Picture::TURNS,
        Picture::TURNS,
    );
}

fn read_zoom(g: &Value) -> Option<f64> {
    value(g, "transformationZoomX")
        .as_f64()
        .filter(|z| z.is_finite())
        .map(|z| rounded(z.clamp(0.25, 4.0)))
}

fn read_rotation(g: &Value) -> Option<f64> {
    value(g, "transformationRotationAngle")
        .as_f64()
        .filter(|r| r.abs() <= Picture::TURNS)
        .map(rounded)
}

pub fn read(item: &Value, reference: &Value, height: u32, origin: i64) -> Option<Title> {
    if let Some(raw) = item["metadata"][VIDCUT].get("title")
        && let Ok(t) = serde_json::from_value::<Title>(raw.clone())
        && let Ok(t) = crate::ops::titles::checked(&t)
    {
        return Some(t);
    }
    let mut t = from_editor(reference, height)?;
    let all = groups(reference);
    if t.kind == TitleKind::Scroll {
        read_scroll_keys(&mut t, &all, origin);
    } else {
        read_rich_keys(&mut t, &all, origin);
    }
    crate::ops::titles::checked(&t).ok()
}

pub fn with_keys_of(mut t: Title, read: &Title) -> Title {
    take_keys(&mut t, read);
    t.opacity = read.opacity;
    t.opacity_keys.clone_from(&read.opacity_keys);
    if read.upper.is_some() {
        t.part_lines();
        t.swap_lines();
        take_keys(&mut t, &read.upper_view());
        t.swap_lines();
    }
    t
}

fn take_keys(t: &mut Title, read: &Title) {
    let dy = t.place()[1] - read.place()[1];
    t.position_keys.clone_from(&read.position_keys);
    t.position_y = read.position_y.iter().map(|y| down(y + dy)).collect();
    t.position_handles.clone_from(&read.position_handles);
    t.zoom_keys.clone_from(&read.zoom_keys);
    t.rotation_keys.clone_from(&read.rotation_keys);
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
    let h = f64::from(height.max(1));
    let marked =
        |r: &Read, base: &Read, usual: f64, tracked: f64, cased: TitleCase| -> Vec<String> {
            r.lines
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    markup(
                        r.spacings.get(i).copied().flatten().filter(|v| *v != usual),
                        tracked,
                        cased,
                        l,
                        base,
                        h,
                    )
                })
                .collect()
        };
    let mut line_spacing = None;
    let tracking;
    let mut case = None;
    let (lines, style_from, at, turned_by) = match kind {
        TitleKind::Scroll => return None,
        TitleKind::Text => {
            let r = read_of(first);
            let at = point(value(first, "position")).unwrap_or([0.5, 0.5]);
            let shared = r.spacings.first().copied().flatten().unwrap_or(0.0);
            let usual = if r.spacings.iter().all(|s| s.unwrap_or(0.0) == shared) {
                shared
            } else {
                0.0
            };
            line_spacing = (usual != 0.0).then_some(usual);
            let mut spans = r.lines.iter().flatten().map(|(_, st)| st.tracking);
            let shared = spans.next().unwrap_or(0.0);
            let tracked = if spans.all(|t| t == shared) {
                shared
            } else {
                0.0
            };
            tracking = (tracked != 0.0).then_some(tracked);
            let mut letters = r.lines.iter().flatten().map(|(_, st)| st.case);
            let first_case = letters.next().unwrap_or_default();
            let cased = if letters.all(|c| c == first_case) {
                first_case
            } else {
                TitleCase::Mixed
            };
            case = (cased != TitleCase::Mixed).then_some(cased);
            (marked(&r, &r, usual, tracked, cased), r, at, first)
        }
        TitleKind::LowerThird => {
            let (upper, lower) = upper_lower(&texts)?;
            let low = read_of(lower);
            let high = upper.map(read_of).unwrap_or_default();
            let style_from = if high.points.is_some() { high } else { low };
            let low = read_of(lower);
            let high = upper.map(read_of).unwrap_or_default();
            let text = |r: &Read, usual| {
                marked(r, &style_from, usual, 0.0, TitleCase::Mixed)
                    .join(" ")
                    .trim()
                    .to_string()
            };
            let lines: Vec<String> = [text(&high, 1.0), text(&low, 0.0)]
                .into_iter()
                .filter(|l| !l.is_empty())
                .collect();
            let at = point(value(lower, "position")).unwrap_or([0.1, 0.22]);
            tracking = Some(0.0);
            (lines, style_from, at, lower)
        }
    };
    let zoom = read_zoom(turned_by);
    let rotation = read_rotation(turned_by);
    let (align, anchor) = if kind == TitleKind::Text {
        (
            style_from.align.filter(|a| *a != TitleAlign::Middle),
            anchor_read(first, TitleAnchor::default()),
        )
    } else {
        let usual = Title::usual_anchor(kind, align.unwrap_or_default(), TitlePart::Lower);
        (align, anchor_read(turned_by, usual))
    };
    let size = style_from
        .points
        .map(|p| p / h)
        .filter(|s| size_fits(*s, Title::MOST_SIZE));
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
                size: Some(steps.min(TextStroke::MOST)),
                outside: value(g, "strokeOutsideOnly").as_bool().filter(|o| *o),
            }
        } else {
            SubtitleOutline {
                color: None,
                size: Some(0.0),
                outside: None,
            }
        }
    });
    let background = backing_of(&all);
    let shadow = shadow_of(&all);
    let mut title = Title {
        kind,
        align,
        anchor,
        text: lines.join("\n"),
        style: TitleStyle {
            font,
            size,
            line_spacing,
            tracking,
            case,
            color,
            outline,
            background,
            shape: shape_of(&all),
            shadow,
        },
        position: None,
        position_keys: Vec::new(),
        position_y: Vec::new(),
        position_handles: Vec::new(),
        zoom,
        zoom_keys: Vec::new(),
        rotation,
        rotation_keys: Vec::new(),
        opacity: None,
        opacity_keys: Vec::new(),
        upper: None,
    };
    let [x, from_bottom] = at;
    title.position = Some([across(x), down(1.0 - from_bottom)]);
    if kind == TitleKind::LowerThird
        && title.text.contains('\n')
        && let Some((Some(high), _)) = upper_lower(&texts)
    {
        let [x, from_bottom] = point(value(high, "position")).unwrap_or([x, 0.2]);
        let usual = Title::usual_anchor(kind, title.align(), TitlePart::Upper);
        let line = TitleLine {
            anchor: anchor_read(high, usual),
            position: Some([across(x), down(1.0 - from_bottom)]),
            zoom: read_zoom(high),
            rotation: read_rotation(high),
            ..TitleLine::default()
        };
        title.upper = Some(line).filter(|u| *u != title.upper_with_lower());
    }
    crate::ops::titles::checked(&title).ok()
}
