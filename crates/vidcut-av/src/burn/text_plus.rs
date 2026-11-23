use cosmic_text::{
    Attrs, Buffer, CacheKeyFlags, Family, FontSystem, Metrics, Shaping, Stretch, Style, Weight,
    Wrap, fontdb,
};
use vidcut_core::model::TextPlus;

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
    numbers: [u64; 13],
    size: (u32, u32),
}

impl Lettering {
    pub fn of(t: &TextPlus, size: (u32, u32)) -> Self {
        let [r, g, b] = t.ink();
        let [across, down] = t.anchors();
        let [x, y] = t.middle();
        let numbers = [
            t.scale(),
            r,
            g,
            b,
            t.alpha(),
            t.track(),
            t.spacing(),
            across,
            down,
            x,
            y,
            t.layout(),
            t.turn(),
        ];
        Self {
            text: t.text.clone(),
            font: t.font.clone(),
            style: t.face().to_string(),
            numbers: numbers.map(f64::to_bits),
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

struct Line {
    glyphs: Vec<([f64; 2], cosmic_text::CacheKey)>,
    end: f64,
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
        r,
        g,
        b,
        opacity,
        tracking,
        spacing,
        across,
        down,
        x,
        y,
        layout,
        turn,
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
    let count = key.text.split('\n').count();
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
            own.glyphs.push(([gx, gy], plain_key(g)));
            own.end = own.end.max(f64::from(g.x + g.w) + k * track);
        }
    }
    let last = -((count - 1) as f64) * pitch;
    let rise = (top + last) / 2.0 - down * (top - last) / 2.0;
    let (sin, cos) = turn.to_radians().sin_cos();
    let middle = [x * w, (1.0 - y) * h];
    let mut shapes = Vec::new();
    for (i, own) in lines.iter().enumerate() {
        let shift = -(1.0 + across) / 2.0 * own.end;
        let base = -(i as f64) * pitch - rise;
        for &([gx, gy], glyph) in &own.glyphs {
            let Some(path) = f.cache.get_outline_commands(&mut f.system, glyph) else {
                continue;
            };
            let at = |p: zeno::Vector| {
                let lx = (shift + gx + f64::from(p.x)) * layout;
                let ly = (base + gy + f64::from(p.y)) * layout;
                zeno::Vector::new(
                    (middle[0] + lx * cos - ly * sin) as f32,
                    (middle[1] - lx * sin - ly * cos) as f32,
                )
            };
            let path = straightened(&transformed(path, at));
            let Some([x0, y0, x1, y1]) = bounds(&path) else {
                continue;
            };
            let clip = |v: f32, most: f64| f64::from(v).clamp(0.0, most) as u32;
            let edges = [
                clip(x0.floor(), w),
                clip(y0.floor(), h),
                clip(x1.ceil(), w),
                clip(y1.ceil(), h),
            ];
            if edges[2] > edges[0] && edges[3] > edges[1] {
                shapes.push((path, edges));
            }
        }
    }
    let [left, up, right, bottom] = shapes.iter().map(|(_, e)| *e).reduce(|a, e| {
        [
            a[0].min(e[0]),
            a[1].min(e[1]),
            a[2].max(e[2]),
            a[3].max(e[3]),
        ]
    })?;
    let (width, height) = (right - left, bottom - up);
    let mut cover = vec![0.0_f32; (width * height) as usize];
    for (path, [x0, y0, x1, y1]) in &shapes {
        let (mw, mh) = (x1 - x0, y1 - y0);
        let (mask, _) = zeno::Mask::new(&path[..])
            .format(zeno::Format::Alpha)
            .origin(zeno::Origin::TopLeft)
            .offset(zeno::Vector::new(-(*x0 as f32), -(*y0 as f32)))
            .size(mw, mh)
            .render();
        for j in 0..mh {
            for i in 0..mw {
                let a = f32::from(mask[(j * mw + i) as usize]) / 255.0;
                let d = &mut cover[((y0 - up + j) * width + x0 - left + i) as usize];
                *d = *d + a - *d * a;
            }
        }
    }
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
