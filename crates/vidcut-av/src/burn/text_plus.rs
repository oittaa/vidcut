use cosmic_text::{
    Attrs, Buffer, CacheKeyFlags, Family, FontSystem, Metrics, Shaping, Stretch, Style, Weight,
    Wrap, fontdb,
};
use vidcut_core::model::{Appearance, BorderLevel, JoinStyle, ShadingElement, TextPlus};

use super::{
    face_of, fonts, installed, installed_usual, plain_key, straightened, with_face_weight,
};

const LINE: f64 = 0.8;

const WEIGHTS: [(&str, u16); 15] = [
    ("extralight", 200),
    ("ultralight", 200),
    ("semibold", 600),
    ("demibold", 600),
    ("extrabold", 800),
    ("ultrabold", 800),
    ("hairline", 100),
    ("thin", 100),
    ("light", 300),
    ("book", 400),
    ("regular", 400),
    ("medium", 500),
    ("bold", 700),
    ("black", 900),
    ("heavy", 900),
];

const STRETCHES: [(&str, Stretch); 8] = [
    ("ultracondensed", Stretch::UltraCondensed),
    ("extracondensed", Stretch::ExtraCondensed),
    ("semicondensed", Stretch::SemiCondensed),
    ("condensed", Stretch::Condensed),
    ("ultraexpanded", Stretch::UltraExpanded),
    ("extraexpanded", Stretch::ExtraExpanded),
    ("semiexpanded", Stretch::SemiExpanded),
    ("expanded", Stretch::Expanded),
];

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Lettering {
    text: String,
    font: Option<String>,
    style: String,
    numbers: [u64; 19],
    look: Appearance,
    join: JoinStyle,
    level: BorderLevel,
    outside: bool,
    size: (u32, u32),
}

impl Lettering {
    pub fn of(t: &TextPlus, s: &ShadingElement, size: (u32, u32)) -> Self {
        let [across, down] = t.anchors();
        let [x, y] = t.middle();
        let [r, g, b] = s.ink();
        let [wide, high] = s.extents();
        let [right, up] = s.shift();
        let numbers = [
            t.scale(),
            t.track(),
            t.spacing(),
            across,
            down,
            x,
            y,
            t.layout(),
            t.turn(),
            r,
            g,
            b,
            s.alpha(),
            s.width(),
            wide,
            high,
            s.rounding(),
            right,
            up,
        ];
        Self {
            text: t.text.clone(),
            font: t.font.clone(),
            style: t.face().to_string(),
            numbers: numbers.map(f64::to_bits),
            look: s.look(),
            join: s.join(),
            level: s.reach(),
            outside: s.outside_only,
            size,
        }
    }
}

pub struct Inked {
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<[f32; 4]>,
}

fn face_parts(style: &str) -> (Weight, Style, Stretch) {
    let name: String = style
        .to_lowercase()
        .chars()
        .filter(char::is_ascii_alphabetic)
        .collect();
    let weight = WEIGHTS
        .iter()
        .find(|(word, _)| name.contains(word))
        .map_or(Weight::NORMAL, |(_, w)| Weight(*w));
    let slant = match (name.contains("italic"), name.contains("oblique")) {
        (true, _) => Style::Italic,
        (false, true) => Style::Oblique,
        _ => Style::Normal,
    };
    let stretch = STRETCHES
        .iter()
        .find(|(word, _)| name.contains(word))
        .map_or(Stretch::Normal, |(_, s)| *s);
    (weight, slant, stretch)
}

fn heights(system: &FontSystem, id: fontdb::ID) -> Option<[f64; 3]> {
    use skrifa::raw::TableProvider;
    system.db().with_face_data(id, |data, index| {
        let font = skrifa::FontRef::from_index(data, index).ok()?;
        let upm = f64::from(font.head().ok()?.units_per_em());
        let hhea = font.hhea().ok()?;
        let ascent = f64::from(hhea.ascender().to_i16());
        let descent = -f64::from(hhea.descender().to_i16());
        (upm > 0.0 && ascent + descent > 0.0).then_some([upm, ascent, descent])
    })?
}

pub fn styles(family: Option<&str>) -> Vec<String> {
    use skrifa::MetadataProvider;
    use skrifa::string::StringId;
    let f = fonts().lock().unwrap_or_else(|e| e.into_inner());
    let Some(name) = family
        .and_then(|n| installed(&f, n))
        .or_else(|| installed_usual(&f.system).map(str::to_string))
    else {
        return Vec::new();
    };
    let db = f.system.db();
    let mut names: Vec<String> = db
        .faces()
        .filter(|face| face.families.iter().any(|(n, _)| *n == name))
        .filter_map(|face| {
            db.with_face_data(face.id, |data, index| {
                let font = skrifa::FontRef::from_index(data, index).ok()?;
                let named = |id: StringId| font.localized_strings(id).english_or_first();
                let own = named(StringId::TYPOGRAPHIC_SUBFAMILY_NAME)
                    .or_else(|| named(StringId::SUBFAMILY_NAME))
                    .map(|s| s.to_string());
                let instances = font.named_instances().iter().filter_map(|i| {
                    font.localized_strings(i.subfamily_name_id())
                        .english_or_first()
                        .map(|s| s.to_string())
                });
                Some(own.into_iter().chain(instances).collect::<Vec<_>>())
            })
            .flatten()
        })
        .flatten()
        .filter(|n| !n.is_empty())
        .collect();
    names.sort_by_key(|n| {
        let (weight, slant, stretch) = face_parts(n);
        (stretch.to_number(), weight.0, slant != Style::Normal)
    });
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    names
}

struct Glyph {
    at: [f64; 2],
    key: cosmic_text::CacheKey,
    advance: f64,
    blank: bool,
}

struct Line {
    glyphs: Vec<Glyph>,
    end: f64,
}

const KAPPA: f64 = 0.552_284_749_830_793_6;

type Reach<'a> = Vec<(&'a [zeno::Command], [u32; 4])>;

fn spans(line: &Line, level: BorderLevel, pad: f64) -> Vec<[f64; 2]> {
    match level {
        BorderLevel::Character => (line.glyphs.iter())
            .filter(|g| !g.blank)
            .map(|g| [g.at[0], g.at[0] + g.advance])
            .collect(),
        BorderLevel::Word => {
            let mut words = Vec::new();
            let mut word: Option<[f64; 2]> = None;
            for g in &line.glyphs {
                let end = g.at[0] + g.advance;
                word = match (g.blank, word) {
                    (true, w) => {
                        words.extend(w);
                        None
                    }
                    (false, Some([start, _])) => Some([start, end]),
                    (false, None) => Some([g.at[0], end]),
                };
            }
            words.extend(word);
            words.iter().map(|[a, b]| [a - pad, b + pad]).collect()
        }
        BorderLevel::Line | BorderLevel::Text => match line.glyphs.iter().any(|g| !g.blank) {
            true => vec![[-pad, line.end + pad]],
            false => Vec::new(),
        },
    }
}

fn boxes(lines: &[(f64, f64, &Line)], level: BorderLevel, [rise, fall]: [f64; 2]) -> Vec<[f64; 4]> {
    let mut out = Vec::new();
    for &(shift, base, line) in lines {
        for [a, b] in spans(line, level, fall) {
            out.push([shift + a, base - fall, shift + b, base + rise]);
        }
    }
    if level != BorderLevel::Text {
        return out;
    }
    let whole = out.into_iter().reduce(|a, b| {
        [
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]
    });
    whole.into_iter().collect()
}

fn rounded([x0, y0, x1, y1]: [f64; 4], r: f64) -> Vec<zeno::Command> {
    use zeno::Command::{Close, CurveTo, LineTo, MoveTo};
    let v = |[x, y]: [f64; 2]| zeno::Vector::new(x as f32, y as f32);
    let corner = |a: [f64; 2], c: [f64; 2], b: [f64; 2]| {
        let toward = |p: [f64; 2]| v([p[0] + (c[0] - p[0]) * KAPPA, p[1] + (c[1] - p[1]) * KAPPA]);
        CurveTo(toward(a), toward(b), v(b))
    };
    vec![
        MoveTo(v([x0 + r, y0])),
        LineTo(v([x1 - r, y0])),
        corner([x1 - r, y0], [x1, y0], [x1, y0 + r]),
        LineTo(v([x1, y1 - r])),
        corner([x1, y1 - r], [x1, y1], [x1 - r, y1]),
        LineTo(v([x0 + r, y1])),
        corner([x0 + r, y1], [x0, y1], [x0, y1 - r]),
        LineTo(v([x0, y0 + r])),
        corner([x0, y0 + r], [x0, y0], [x0 + r, y0]),
        Close,
    ]
}

fn joined(join: JoinStyle) -> zeno::Join {
    match join {
        JoinStyle::Sharp => zeno::Join::Miter,
        JoinStyle::Rounded => zeno::Join::Round,
        JoinStyle::Beveled => zeno::Join::Bevel,
    }
}

fn reach(shapes: &[Vec<zeno::Command>], pad: f64, (w, h): (f64, f64)) -> Reach<'_> {
    let clip = |v: f64, most: f64| v.clamp(0.0, most) as u32;
    shapes
        .iter()
        .filter_map(|path| {
            let [x0, y0, x1, y1] = bounds(path)?.map(f64::from);
            let edges = [
                clip((x0 - pad).floor(), w),
                clip((y0 - pad).floor(), h),
                clip((x1 + pad).ceil(), w),
                clip((y1 + pad).ceil(), h),
            ];
            (edges[2] > edges[0] && edges[3] > edges[1]).then_some((&path[..], edges))
        })
        .collect()
}

fn union(reach: &Reach<'_>) -> Option<[u32; 4]> {
    reach.iter().map(|(_, e)| *e).reduce(|a, e| {
        [
            a[0].min(e[0]),
            a[1].min(e[1]),
            a[2].max(e[2]),
            a[3].max(e[3]),
        ]
    })
}

fn painted(reach: &Reach<'_>, all: [u32; 4], stroke: Option<zeno::Stroke>) -> Vec<f32> {
    let width = all[2] - all[0];
    let mut cover = vec![0.0_f32; (width * (all[3] - all[1])) as usize];
    for &(path, [x0, y0, x1, y1]) in reach {
        let (mw, mh) = (x1 - x0, y1 - y0);
        let mut mask = zeno::Mask::new(path);
        mask.format(zeno::Format::Alpha)
            .origin(zeno::Origin::TopLeft)
            .offset(zeno::Vector::new(-(x0 as f32), -(y0 as f32)))
            .size(mw, mh);
        if let Some(s) = stroke {
            mask.style(s);
        }
        let (alphas, _) = mask.render();
        for j in 0..mh {
            for i in 0..mw {
                let a = f32::from(alphas[(j * mw + i) as usize]) / 255.0;
                let d = &mut cover[((y0 - all[1] + j) * width + x0 - all[0] + i) as usize];
                *d = *d + a - *d * a;
            }
        }
    }
    cover
}

fn transformed(
    path: &[zeno::Command],
    at: impl Fn(zeno::Vector) -> zeno::Vector,
) -> Vec<zeno::Command> {
    use zeno::Command::{Close, CurveTo, LineTo, MoveTo, QuadTo};
    path.iter()
        .map(|c| match *c {
            MoveTo(p) => MoveTo(at(p)),
            LineTo(p) => LineTo(at(p)),
            QuadTo(c, p) => QuadTo(at(c), at(p)),
            CurveTo(c, d, p) => CurveTo(at(c), at(d), at(p)),
            Close => Close,
        })
        .collect()
}

fn bounds(path: &[zeno::Command]) -> Option<[f32; 4]> {
    use zeno::Command::{Close, CurveTo, LineTo, MoveTo, QuadTo};
    let points = path.iter().flat_map(|c| match *c {
        MoveTo(p) | LineTo(p) => vec![p],
        QuadTo(c, p) => vec![c, p],
        CurveTo(c, d, p) => vec![c, d, p],
        Close => Vec::new(),
    });
    points.fold(None, |b, p| {
        let [x0, y0, x1, y1] = b.unwrap_or([p.x, p.y, p.x, p.y]);
        Some([x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y)])
    })
}

pub fn inked(key: &Lettering) -> Option<Inked> {
    let [
        size,
        tracking,
        spacing,
        across,
        down,
        x,
        y,
        layout,
        turn,
        r,
        g,
        b,
        opacity,
        thickness,
        wide,
        high,
        round,
        right,
        up,
    ] = key.numbers.map(f64::from_bits);
    let (w, h) = (f64::from(key.size.0), f64::from(key.size.1));
    if key.text.is_empty() || size <= 0.0 || layout <= 0.0 || opacity <= 0.0 {
        return None;
    }
    let mut held = fonts().lock().unwrap_or_else(|e| e.into_inner());
    let f = &mut *held;
    f.limit_glyphs();
    let named = key.font.as_deref().and_then(|n| installed(f, n));
    let family = named.as_deref().map_or(Family::SansSerif, Family::Name);
    let (weight, slant, stretch) = face_parts(&key.style);
    let attrs = Attrs::new()
        .family(family)
        .weight(weight)
        .style(slant)
        .stretch(stretch)
        .cache_key_flags(CacheKeyFlags::DISABLE_HINTING);
    let attrs = with_face_weight(&f.system, attrs);
    let [upm, ascent, descent] = heights(&f.system, face_of(&f.system, &attrs)?)?;
    let line = LINE * size * w;
    let em = line * upm / (ascent + descent);
    let top = em - em * descent / upm;
    let pitch = line * spacing;
    let track = (tracking - 1.0) * size * w;
    let mut buffer = Buffer::new(&mut f.system, Metrics::new(em as f32, line as f32));
    buffer.set_wrap(Wrap::None);
    buffer.set_text(&key.text, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(&mut f.system, false);
    let count = buffer.lines.len().max(1);
    let mut lines: Vec<Line> = (0..count)
        .map(|_| Line {
            glyphs: Vec::new(),
            end: 0.0,
        })
        .collect();
    for run in buffer.layout_runs() {
        let Some(own) = lines.get_mut(run.line_i) else {
            continue;
        };
        for g in run.glyphs {
            let k = own.glyphs.len() as f64;
            let gx = f64::from(g.x + g.x_offset * g.font_size) + k * track;
            let gy = f64::from(g.y_offset * g.font_size - g.y);
            let typed = run.text.get(g.start..g.end).unwrap_or_default();
            own.glyphs.push(Glyph {
                at: [gx, gy],
                key: plain_key(g),
                advance: f64::from(g.w),
                blank: typed.chars().all(char::is_whitespace),
            });
            own.end = own.end.max(f64::from(g.x + g.w) + k * track);
        }
    }
    let last = -((count - 1) as f64) * pitch;
    let rise = (top + last) / 2.0 - down * (top - last) / 2.0;
    let (sin, cos) = turn.to_radians().sin_cos();
    let middle = [x * w, (1.0 - y) * h];
    let unit = size * w;
    let place = |p: zeno::Vector| {
        let lx = (f64::from(p.x) + right * unit) * layout;
        let ly = (f64::from(p.y) + up * unit) * layout;
        zeno::Vector::new(
            (middle[0] + lx * cos - ly * sin) as f32,
            (middle[1] - lx * sin - ly * cos) as f32,
        )
    };
    let placed: Vec<(f64, f64, &Line)> = (lines.iter().enumerate())
        .map(|(i, own)| {
            let shift = -(1.0 + across) / 2.0 * own.end;
            (shift, -(i as f64) * pitch - rise, own)
        })
        .collect();
    let shapes: Vec<Vec<zeno::Command>> = match key.look {
        Appearance::TextFill | Appearance::TextOutline => {
            let mut shapes = Vec::new();
            for &(shift, base, own) in &placed {
                for glyph in &own.glyphs {
                    let Some(path) = f.cache.get_outline_commands(&mut f.system, glyph.key) else {
                        continue;
                    };
                    let [gx, gy] = glyph.at;
                    let at = |p: zeno::Vector| {
                        let x = (shift + gx + f64::from(p.x)) as f32;
                        place(zeno::Vector::new(x, (base + gy + f64::from(p.y)) as f32))
                    };
                    shapes.push(straightened(&transformed(path, at)));
                }
            }
            shapes
        }
        Appearance::BorderFill | Appearance::BorderOutline => {
            let heights = [ascent, descent].map(|v| em * v / upm);
            let grow = [wide, high].map(|e| ShadingElement::EXTEND * e * unit);
            boxes(&placed, key.level, heights)
                .into_iter()
                .map(|[x0, y0, x1, y1]| [x0 - grow[0], y0 - grow[1], x1 + grow[0], y1 + grow[1]])
                .filter(|[x0, y0, x1, y1]| x1 > x0 && y1 > y0)
                .map(|b| {
                    let r = (round * unit)
                        .min((b[2] - b[0]) / 2.0)
                        .min((b[3] - b[1]) / 2.0);
                    straightened(&transformed(&rounded(b, r.max(0.0)), place))
                })
                .collect()
        }
    };
    let outlined = matches!(
        key.look,
        Appearance::TextOutline | Appearance::BorderOutline
    );
    let pen = thickness * unit * layout;
    let mut stroke = zeno::Stroke::new(pen as f32);
    stroke
        .join(joined(key.join))
        .miter_limit(ShadingElement::MITER as f32);
    let corner = match key.join {
        JoinStyle::Sharp => ShadingElement::MITER,
        _ => 1.0,
    };
    let pad = match outlined {
        true => pen / 2.0 * corner + 1.0,
        false => 0.0,
    };
    let edges = reach(&shapes, pad, (w, h));
    let all = union(&edges)?;
    let cover = match (outlined, key.outside) {
        (false, _) => painted(&edges, all, None),
        (true, false) => painted(&edges, all, Some(stroke)),
        (true, true) => {
            let line = painted(&edges, all, Some(stroke));
            let fill = painted(&reach(&shapes, 0.0, (w, h)), all, None);
            (line.iter().zip(&fill))
                .map(|(l, f)| l.max(*f) - f)
                .collect()
        }
    };
    let [left, up, right, bottom] = all;
    let (width, height) = (right - left, bottom - up);
    let ink = [r, g, b].map(|c| c as f32);
    let alpha = opacity as f32;
    let rgba = cover
        .into_iter()
        .map(|c| {
            let a = c * alpha;
            [ink[0] * a, ink[1] * a, ink[2] * a, a]
        })
        .collect();
    Some(Inked {
        left,
        top: up,
        width,
        height,
        rgba,
    })
}
