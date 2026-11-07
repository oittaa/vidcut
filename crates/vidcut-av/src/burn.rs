use std::sync::{Mutex, MutexGuard, OnceLock};

use cosmic_text::{
    Align, Attrs, Buffer, CacheKey, CacheKeyFlags, Color, Family, Font, FontSystem, LayoutGlyph,
    LayoutRun, Metrics, Renderer, Shaping, Style, SwashCache, SwashContent, UnderlineStyle, Weight,
    Wrap, fontdb,
};
use vidcut_core::lru::ByBytes;
use vidcut_core::model::{
    Place, SubtitleAlign, SubtitleStyle, TextStroke, Title, TitleAlign, TitleBlock, TitleCase,
    TitleKind, TitlePart, TitleShadow, TitleStyle,
};
use vidcut_core::rich::Shift;
use vidcut_core::subtitles::{Piece, Shown, rgb};

const CHORD_LIMIT: f32 = 1.0 / 6.0;
const CHORD_SHORTEST: f32 = 1.0 / 256.0;
const FAMILIES: &[&str] = &[
    "Open Sans",
    "Segoe UI",
    "Helvetica Neue",
    "Noto Sans",
    "DejaVu Sans",
    "Arial",
    "Liberation Sans",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Sprite {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub lift: f32,
    pub float: bool,
    pub rgba: Vec<[f32; 4]>,
}

impl Sprite {
    fn at(&self, x: i32, y: i32) -> [f32; 4] {
        let (sx, sy) = (x - self.x, y - self.y);
        if sx < 0 || sy < 0 || sx >= self.width as i32 || sy >= self.height as i32 {
            return [0.0; 4];
        }
        self.rgba[sy as usize * self.width as usize + sx as usize]
    }
}

const GLYPH_BYTES: usize = 64 << 20;

struct Fonts {
    system: FontSystem,
    cache: SwashCache,
}

impl Fonts {
    fn limit_glyphs(&mut self) {
        let images = self.cache.image_cache.values().flatten();
        let outlines = self.cache.outline_command_cache.values().flatten();
        let bytes = images.map(|i| i.data.len()).sum::<usize>()
            + outlines.map(|o| size_of_val(&**o)).sum::<usize>();
        if bytes > GLYPH_BYTES {
            self.cache.image_cache.clear();
            self.cache.outline_command_cache.clear();
        }
    }
}

fn installed_usual(system: &FontSystem) -> Option<&'static str> {
    FAMILIES.iter().copied().find(|name| {
        system
            .db()
            .faces()
            .any(|f| f.families.iter().any(|(n, _)| n == name))
    })
}

fn family_names(data: &[u8], index: u32) -> Vec<String> {
    use skrifa::MetadataProvider;
    use skrifa::string::StringId;
    skrifa::FontRef::from_index(data, index).map_or(Vec::new(), |font| {
        font.localized_strings(StringId::FAMILY_NAME)
            .map(|name| name.to_string())
            .filter(|name| !name.is_empty())
            .collect()
    })
}

fn add_family_names(db: &mut fontdb::Database) {
    let named: Vec<fontdb::FaceInfo> = db
        .faces()
        .filter_map(|face| {
            let mut named = face.clone();
            for name in db.with_face_data(face.id, family_names)? {
                if !named.families.iter().any(|(n, _)| *n == name) {
                    named.families.push((name, fontdb::Language::Unknown));
                }
            }
            (named.families.len() > face.families.len()).then_some(named)
        })
        .collect();
    for face in named {
        db.remove_face(face.id);
        db.push_face_info(face);
    }
}

fn fonts() -> &'static Mutex<Fonts> {
    static FONTS: OnceLock<Mutex<Fonts>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let (locale, mut db) = FontSystem::new().into_locale_and_db();
        add_family_names(&mut db);
        let mut system = FontSystem::new_with_locale_and_db(locale, db);
        if let Some(name) = installed_usual(&system) {
            system.db_mut().set_sans_serif_family(name);
        }
        Mutex::new(Fonts {
            system,
            cache: SwashCache::new(),
        })
    })
}

pub fn usual_family() -> Option<String> {
    let f = fonts().lock().unwrap_or_else(|e| e.into_inner());
    installed_usual(&f.system).map(str::to_string)
}

pub fn families() -> Vec<String> {
    let f = fonts().lock().unwrap_or_else(|e| e.into_inner());
    let mut names: Vec<String> = f
        .system
        .db()
        .faces()
        .filter_map(|face| face.families.first().map(|(n, _)| n.clone()))
        .collect();
    names.sort_by_cached_key(|n| n.to_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    names
}

fn color([r, g, b]: [u8; 3]) -> Color {
    Color::rgb(r, g, b)
}

fn unit([r, g, b]: [u8; 3], a: f32) -> [f32; 4] {
    [
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        a,
    ]
}

const HIDDEN: usize = 1;
const POPPED: usize = 2;
const RAISED: usize = 4;
const LOWERED: usize = 8;
const FULL_SIZE_BIT: u32 = 4;
const LTSH_HEAD: usize = 4;

fn spans<'a>(
    system: &FontSystem,
    pieces: &[Piece],
    family: Family<'a>,
    faces: &'a [Option<String>],
    ink: Color,
    size: f32,
    line: f32,
) -> Vec<(String, Attrs<'a>)> {
    pieces
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut a = Attrs::new()
                .family(
                    faces
                        .get(i)
                        .and_then(|n| n.as_deref())
                        .map_or(family, Family::Name),
                )
                .color(p.color.as_deref().and_then(rgb).map_or(ink, color));
            if let Some(scale) = p.size_scale {
                let px = (size * scale as f32 / 1000.0).max(1.0);
                a = a.metrics(Metrics::new(px, px * line));
            }
            if p.italic {
                a = a.style(Style::Italic);
            }
            if p.bold {
                a = a.weight(Weight::BOLD);
            }
            if p.underline {
                a = a.underline(UnderlineStyle::Single);
            }
            if p.hidden || p.popped {
                a = a
                    .color(Color::rgba(0, 0, 0, 0))
                    .metadata(HIDDEN | if p.popped { POPPED } else { 0 });
            }
            (p.text.clone(), with_face_weight(system, a))
        })
        .collect()
}

fn dilate(alpha: &[f32], w: usize, h: usize, radius: f32) -> Vec<f32> {
    let reach = radius.ceil() as i32 + 1;
    let disk: Vec<(i32, i32, f32)> = (-reach..=reach)
        .flat_map(|dy| (-reach..=reach).map(move |dx| (dx, dy)))
        .filter_map(|(dx, dy)| {
            let weight = (radius + 0.5 - ((dx * dx + dy * dy) as f32).sqrt()).clamp(0.0, 1.0);
            (weight > 0.0 && (dx, dy) != (0, 0)).then_some((dx, dy, weight))
        })
        .collect();
    let (wi, hi) = (w as i32, h as i32);
    let mut out = alpha.to_vec();
    for sy in 0..hi {
        for sx in 0..wi {
            let a = alpha[(sy * wi + sx) as usize];
            if a <= 0.0 {
                continue;
            }
            for &(dx, dy, weight) in &disk {
                let (tx, ty) = (sx - dx, sy - dy);
                if tx < 0 || ty < 0 || tx >= wi || ty >= hi {
                    continue;
                }
                let t = (ty * wi + tx) as usize;
                let v = a * weight;
                if v > out[t] {
                    out[t] = v;
                }
            }
        }
    }
    out
}

fn over(dst: [f32; 4], src: [f32; 4]) -> [f32; 4] {
    let a = src[3] + dst[3] * (1.0 - src[3]);
    if a <= 0.0 {
        return [0.0; 4];
    }
    let mut out = [0.0; 4];
    for i in 0..3 {
        out[i] = (src[i] * src[3] + dst[i] * dst[3] * (1.0 - src[3])) / a;
    }
    out[3] = a;
    out
}

fn eight_bit(p: [f32; 4]) -> [u8; 4] {
    let a = p[3].clamp(0.0, 1.0);
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [byte(p[0] * a), byte(p[1] * a), byte(p[2] * a), byte(a)]
}

fn stepped(p: [f32; 4]) -> [f32; 4] {
    let [r, g, b, a] = eight_bit(p).map(|v| f32::from(v) / 255.0);
    if a <= 0.0 {
        return [0.0; 4];
    }
    [r / a, g / a, b / a, a]
}

struct Look<'a> {
    font: Option<&'a str>,
    size: f32,
    line: f32,
    title_gap: Option<(&'a Title, f64)>,
    seat: Option<Seat>,
    spacing: (f32, f32),
    tracking: f32,
    case: TitleCase,
    wrap: f32,
    align: Align,
    ink: [u8; 3],
    stroke: Option<Edge>,
    backing: Option<([u8; 3], f64)>,
    frame: Frame,
    shadow: Option<Shade>,
    markup: Markup<'a>,
    bare: bool,
    drop: f32,
    float: bool,
}

#[derive(Debug, Clone, Copy)]
struct Seat {
    middle: f32,
    edge: f32,
    part: TitlePart,
    anchor: [f64; 2],
}

#[derive(Debug, Clone, Copy)]
struct Edge {
    ink: [u8; 3],
    width: f32,
    outside: bool,
}

impl Edge {
    fn of(s: TextStroke, pixel: f64) -> Self {
        Self {
            ink: s.ink,
            width: (s.width * pixel) as f32,
            outside: s.outside,
        }
    }

    fn past(&self) -> f32 {
        self.width / 2.0
    }
}

#[derive(Debug, Clone, Copy)]
struct Frame {
    across: f32,
    down: f32,
    corner: f32,
}

#[derive(Debug, Clone, Copy)]
enum Markup<'a> {
    Pieces(&'a [Piece]),
    Title(&'a str, f32),
}

#[derive(Clone)]
struct Laid {
    tall: f32,
    sprite: Option<Sprite>,
    lines: f32,
    above: f32,
    popped: Option<(f32, f32)>,
    seated: Option<[f32; 2]>,
}

struct Canvas {
    pixels: Vec<[f32; 4]>,
    width: i32,
    height: i32,
    left: i32,
    top: i32,
}

impl Canvas {
    fn index(&self, x: i32, y: i32) -> Option<usize> {
        let (cx, cy) = (x - self.left, y - self.top);
        (cx >= 0 && cy >= 0 && cx < self.width && cy < self.height)
            .then(|| (cy * self.width + cx) as usize)
    }

    fn rectangle(&mut self, x: i32, y: i32, bw: u32, bh: u32, color: Color) {
        let a = f32::from(color.a()) / 255.0;
        if a <= 0.0 {
            return;
        }
        let rgb = [color.r(), color.g(), color.b()].map(|c| f32::from(c) / 255.0);
        for py in y..y + bh as i32 {
            for px in x..x + bw as i32 {
                if let Some(i) = self.index(px, py) {
                    let d = &mut self.pixels[i];
                    for c in 0..3 {
                        d[c] = rgb[c] * a + d[c] * (1.0 - a);
                    }
                    d[3] = a + d[3] * (1.0 - a);
                }
            }
        }
    }

    fn fill(&mut self, t: &Traced, ink: Color) {
        let (mask, at) = masked(t, None);
        let (mw, mh) = (at.width as i32, at.height as i32);
        for j in 0..mh {
            for i in 0..mw {
                let a = u32::from(mask[(j * mw + i) as usize]) * u32::from(ink.a()) / 255;
                let shade = Color::rgba(ink.r(), ink.g(), ink.b(), a as u8);
                self.rectangle(t.x + at.left + i, t.y - at.top + j, 1, 1, shade);
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Line {
    pieces: Vec<Piece>,
    place: Place,
    size: (u32, u32),
    timeline: u32,
    style: SubtitleStyle,
}

const LINES_BYTES: usize = 64 << 20;

const LINE_ENTRY_BYTES: usize = 4 << 10;

fn lines() -> MutexGuard<'static, ByBytes<Line, Laid>> {
    static LINES_KEPT: Mutex<ByBytes<Line, Laid>> = Mutex::new(ByBytes::new());
    LINES_KEPT.lock().unwrap_or_else(|e| e.into_inner())
}

fn laid_line(line: Line, look: &Look) -> Option<Laid> {
    if let Some(laid) = lines().get(&line) {
        return Some(laid.clone());
    }
    let laid = lay_out(look)?;
    let pixels = laid.sprite.as_ref().map_or(0, |s| s.rgba.len());
    let bytes = pixels * size_of::<[f32; 4]>() + LINE_ENTRY_BYTES;
    let mut kept = lines();
    kept.insert(line, laid.clone(), bytes);
    kept.fit(LINES_BYTES);
    Some(laid)
}

fn installed(f: &Fonts, name: &str) -> Option<String> {
    f.system.db().faces().find_map(|face| {
        face.families
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(n, _)| n.clone())
    })
}

#[derive(Debug, Clone, Copy)]
struct Shade {
    ink: [u8; 3],
    opacity: f32,
    shift: (i32, i32),
    sigma: f32,
}

fn shade(s: &TitleShadow, w: f64, h: f64) -> Option<Shade> {
    let [dx, dy] = s.shift();
    (!s.is_off() && (dx != 0.0 || dy != 0.0)).then(|| Shade {
        ink: s.ink(),
        opacity: s.alpha() as f32,
        shift: ((dx * w).round() as i32, (dy * h).round() as i32),
        sigma: (s.softness() * h / TitleShadow::BLUR_PER_PICTURE) as f32,
    })
}

fn radii(sigma: f32) -> [usize; 3] {
    let n = 3.0;
    let ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut low = ideal.floor() as i32;
    if low % 2 == 0 {
        low -= 1;
    }
    let low = low.max(1);
    let lw = low as f32;
    let m =
        ((12.0 * sigma * sigma - n * lw * lw - 4.0 * n * lw - 3.0 * n) / (-4.0 * lw - 4.0)).round();
    let mut out = [0; 3];
    for (i, r) in out.iter_mut().enumerate() {
        let width = if (i as f32) < m { low } else { low + 2 };
        *r = ((width - 1) / 2) as usize;
    }
    out
}

fn box_pass(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize, across: bool) {
    let (lines, len, step, stride) = if across { (h, w, 1, w) } else { (w, h, w, 1) };
    let scale = 1.0 / (2 * r + 1) as f32;
    for l in 0..lines {
        let base = l * stride;
        let at = |i: isize| -> f32 {
            if i < 0 || i >= len as isize {
                0.0
            } else {
                src[base + i as usize * step]
            }
        };
        let mut sum: f32 = (-(r as isize)..=r as isize).map(at).sum();
        for i in 0..len {
            dst[base + i * step] = sum * scale;
            sum += at(i as isize + r as isize + 1) - at(i as isize - r as isize);
        }
    }
}

fn soften(alpha: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let mut a = alpha.to_vec();
    if sigma < 0.5 {
        return a;
    }
    let mut b = vec![0.0; a.len()];
    for r in radii(sigma) {
        box_pass(&a, &mut b, w, h, r, true);
        box_pass(&b, &mut a, w, h, r, false);
    }
    a
}

fn shifted(alpha: &[f32], w: usize, h: usize, (dx, dy): (i32, i32)) -> Vec<f32> {
    let mut out = vec![0.0; alpha.len()];
    for y in 0..h as i32 {
        let sy = y - dy;
        if sy < 0 || sy >= h as i32 {
            continue;
        }
        for x in 0..w as i32 {
            let sx = x - dx;
            if sx >= 0 && sx < w as i32 {
                out[y as usize * w + x as usize] = alpha[sy as usize * w + sx as usize];
            }
        }
    }
    out
}

struct Traced {
    x: i32,
    y: i32,
    path: Vec<zeno::Command>,
    offset: zeno::Vector,
}

struct Bars(Vec<Traced>);

impl Renderer for Bars {
    fn rectangle(&mut self, x: i32, y: i32, w: u32, h: u32, color: Color) {
        use zeno::Command::{Close, LineTo, MoveTo};
        if color.a() == 0 {
            return;
        }
        let (w, h) = (w as f32, h as f32);
        self.0.push(Traced {
            x,
            y,
            path: vec![
                MoveTo([0.0, 0.0].into()),
                LineTo([w, 0.0].into()),
                LineTo([w, -h].into()),
                LineTo([0.0, -h].into()),
                Close,
            ],
            offset: zeno::Vector::ZERO,
        });
    }

    fn glyph(&mut self, _: cosmic_text::PhysicalGlyph, _: Color) {}
}

struct TextLine {
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
    color: Color,
}

fn text_lines(
    system: &mut FontSystem,
    run: &LayoutRun<'_>,
    (lift, freed): (&[f32], &[f32]),
    (scale, origin, nudge, ceiling): (f32, f32, f32, f32),
    ink: Color,
) -> Vec<TextLine> {
    let mut lines = Vec::new();
    let line_freed = freed.get(run.line_i).copied().unwrap_or(0.0);
    let block_left = origin + nudge;
    let end = |x: f32| block_left + Title::line_end(f64::from(x - origin), f64::from(scale)) as f32;
    for span in run.decorations {
        let glyphs = &run.glyphs[span.glyph_range.clone()];
        let (x0, x1) = glyphs.iter().fold((f32::MAX, f32::MIN), |(a, b), g| {
            (a.min(g.x), b.max(g.x + g.w))
        });
        let end_space = glyphs
            .last()
            .filter(|g| g.end == run.text.len())
            .map_or(0.0, |g| line_freed * g.font_size);
        let x1 = x1 - end_space;
        if x1 <= x0 {
            continue;
        }
        let baseline = run.line_y + lift.get(span.glyph_range.start).copied().unwrap_or(0.0);
        let deco = &span.data;
        let full = deco.underline_metrics.thickness * span.font_size;
        let ascent = glyphs
            .first()
            .and_then(|g| font_heights(system, g.font_id, g.font_weight, true))
            .map_or(deco.ascent, |[ascent, ..]| ascent as f32)
            * span.font_size;
        let mut line = |middle: f32| {
            lines.push(TextLine {
                left: end(x0).max(block_left.ceil()),
                right: end(x1),
                top: (middle - full / 2.0).max(ceiling),
                bottom: middle + full / 2.0,
                color: span.color_opt.unwrap_or(ink),
            });
        };
        if deco.text_decoration.underline != UnderlineStyle::None {
            let drop = Title::underline_drop(f64::from(full / scale)) as f32;
            line(baseline + drop * scale);
        }
        if deco.text_decoration.strikethrough {
            line(baseline - ascent / 3.0);
        }
        if deco.text_decoration.overline {
            line(baseline - ascent);
        }
    }
    lines
}

fn decorate<R: Renderer>(
    renderer: &mut R,
    run: &LayoutRun<'_>,
    ink: Color,
    lines: Option<&[TextLine]>,
) {
    let Some(lines) = lines else {
        cosmic_text::render_decoration(renderer, run, ink);
        return;
    };
    for line in lines {
        let color = line.color;
        let (whole_from, whole_to) = (line.left.ceil(), line.right.floor());
        let mut row = line.top.floor();
        while row < line.bottom {
            let cover = (row + 1.0).min(line.bottom) - row.max(line.top);
            let shade = |share: f32| {
                let alpha = f32::from(color.a()) * cover * share;
                Color::rgba(color.r(), color.g(), color.b(), alpha as u8)
            };
            let y = row as i32;
            if whole_to < whole_from {
                renderer.rectangle(
                    line.left.floor() as i32,
                    y,
                    1,
                    1,
                    shade(line.right - line.left),
                );
            } else {
                renderer.rectangle(
                    whole_from as i32 - 1,
                    y,
                    1,
                    1,
                    shade(whole_from - line.left),
                );
                renderer.rectangle(
                    whole_from as i32,
                    y,
                    (whole_to - whole_from) as u32,
                    1,
                    shade(1.0),
                );
                renderer.rectangle(whole_to as i32, y, 1, 1, shade(line.right - whole_to));
            }
            row += 1.0;
        }
    }
}

fn plain_key(g: &LayoutGlyph) -> CacheKey {
    let at = (0.0, 0.0);
    CacheKey::new(
        g.font_id,
        g.glyph_id,
        g.font_size,
        at,
        g.font_weight,
        g.cache_key_flags,
    )
    .0
}

fn letter(g: &LayoutGlyph, (x, y): (f32, f32), f: &mut Fonts) -> Option<Traced> {
    let path = f.cache.get_outline_commands(&mut f.system, plain_key(g))?;
    let x = x + g.x + g.x_offset * g.font_size;
    let y = y + g.y - g.y_offset * g.font_size;
    let (across, down) = (x.floor(), y.ceil());
    Some(Traced {
        x: across as i32,
        y: down as i32,
        path: straightened(path),
        offset: zeno::Vector::new(x - across, down - y),
    })
}

fn letters(
    canvas: &mut Canvas,
    runs: &[LayoutRun<'_>],
    lifts: &[Vec<f32>],
    nudge: [f32; 2],
    white: Color,
    f: &mut Fonts,
) {
    for (run, lift) in runs.iter().zip(lifts) {
        for (g, up) in run
            .glyphs
            .iter()
            .zip(lift)
            .filter(|(g, _)| g.metadata & HIDDEN == 0)
        {
            let ink = g.color_opt.unwrap_or(white);
            let colored = f
                .cache
                .get_image(&mut f.system, plain_key(g))
                .as_ref()
                .is_some_and(|i| i.content == SwashContent::Color);
            let traced = (!colored)
                .then(|| letter(g, (nudge[0], run.line_y + nudge[1] + up), f))
                .flatten();
            let Some(t) = traced else {
                let p = g.physical((nudge[0], run.line_y + up), 1.0);
                f.cache
                    .with_pixels(&mut f.system, p.cache_key, ink, |x, y, c| {
                        canvas.rectangle(p.x + x, p.y + y, 1, 1, c);
                    });
                continue;
            };
            canvas.fill(&t, ink);
        }
    }
}

fn traces(
    runs: &[LayoutRun<'_>],
    lifts: &[Vec<f32>],
    exact: Option<[f32; 2]>,
    lines: Option<&[Vec<TextLine>]>,
    f: &mut Fonts,
) -> Option<Vec<Traced>> {
    let mut bars = Bars(Vec::new());
    for (i, (run, lift)) in runs.iter().zip(lifts).enumerate() {
        for (g, up) in run
            .glyphs
            .iter()
            .zip(lift)
            .filter(|(g, _)| g.metadata & HIDDEN == 0)
        {
            if let Some(nudge) = exact {
                bars.0
                    .push(letter(g, (nudge[0], run.line_y + nudge[1] + up), f)?);
                continue;
            }
            let p = g.physical((0., run.line_y + up), 1.0);
            let path = f.cache.get_outline_commands(&mut f.system, p.cache_key)?;
            bars.0.push(Traced {
                x: p.x,
                y: p.y,
                path: path.to_vec(),
                offset: zeno::Vector::new(
                    p.cache_key.x_bin.as_float(),
                    p.cache_key.y_bin.as_float(),
                ),
            });
        }
        let own = lines.map(|all| all[i].as_slice());
        decorate(&mut bars, run, Color::rgb(255, 255, 255), own);
    }
    Some(bars.0)
}

fn chords(
    a: zeno::Vector,
    b: zeno::Vector,
    c: zeno::Vector,
    d: zeno::Vector,
    out: &mut Vec<zeno::Command>,
) {
    let (dx, dy) = (d.x - a.x, d.y - a.y);
    let (wide, tall) = (dx.abs(), dy.abs());
    let long = if wide > tall {
        wide + 3.0 * tall / 8.0
    } else {
        tall + 3.0 * wide / 8.0
    };
    let limit = long * CHORD_LIMIT;
    let (x1, y1) = (b.x - a.x, b.y - a.y);
    let (x2, y2) = (c.x - a.x, c.y - a.y);
    let flat = (dy * x1 - dx * y1).abs() <= limit
        && (dy * x2 - dx * y2).abs() <= limit
        && x1 * (x1 - dx) + y1 * (y1 - dy) <= 0.0
        && x2 * (x2 - dx) + y2 * (y2 - dy) <= 0.0;
    if flat || long < CHORD_SHORTEST {
        out.push(zeno::Command::LineTo(d));
        return;
    }
    let half =
        |p: zeno::Vector, q: zeno::Vector| zeno::Vector::new((p.x + q.x) / 2.0, (p.y + q.y) / 2.0);
    let (ab, bc, cd) = (half(a, b), half(b, c), half(c, d));
    let (abc, bcd) = (half(ab, bc), half(bc, cd));
    let middle = half(abc, bcd);
    chords(a, ab, abc, middle, out);
    chords(middle, bcd, cd, d, out);
}

fn straightened(path: &[zeno::Command]) -> Vec<zeno::Command> {
    use zeno::Command::{Close, CurveTo, LineTo, MoveTo, QuadTo};
    let mut out = Vec::with_capacity(path.len() * 4);
    let mut now = zeno::Vector::ZERO;
    for command in path {
        match *command {
            MoveTo(p) => {
                out.push(MoveTo(p));
                now = p;
            }
            LineTo(p) => {
                out.push(LineTo(p));
                now = p;
            }
            QuadTo(c, p) => {
                let near = |e: zeno::Vector| {
                    zeno::Vector::new(e.x + 2.0 * (c.x - e.x) / 3.0, e.y + 2.0 * (c.y - e.y) / 3.0)
                };
                chords(now, near(now), near(p), p, &mut out);
                now = p;
            }
            CurveTo(c, d, p) => {
                chords(now, c, d, p, &mut out);
                now = p;
            }
            Close => out.push(Close),
        }
    }
    out
}

fn masked(t: &Traced, pen: Option<f32>) -> (Vec<u8>, zeno::Placement) {
    use zeno::{Cap, Format, Join, Mask, Origin, Stroke};
    let mut mask = Mask::new(&t.path[..]);
    if let Some(width) = pen {
        let mut stroke = Stroke::new(width);
        stroke.join(Join::Round).cap(Cap::Round);
        mask.style(stroke);
    }
    mask.format(Format::Alpha)
        .origin(Origin::BottomLeft)
        .offset(t.offset)
        .render_offset(t.offset)
        .inspect(|_, _, _| ());
    mask.render()
}

fn stroked(traces: &[Traced], width: f32, into: &mut [f32], w: usize, (left, top): (i32, i32)) {
    let h = into.len() / w;
    for t in traces {
        let (mask, at) = masked(t, Some(width));
        let (mw, mh) = (at.width as i32, at.height as i32);
        for j in 0..mh {
            let y = t.y - at.top + j - top;
            if y < 0 || y >= h as i32 {
                continue;
            }
            for i in 0..mw {
                let x = t.x + at.left + i - left;
                if x < 0 || x >= w as i32 {
                    continue;
                }
                let a = f32::from(mask[(j * mw + i) as usize]) / 255.0;
                let d = &mut into[y as usize * w + x as usize];
                if a > *d {
                    *d = a;
                }
            }
        }
    }
}

fn covered((x0, y0, x1, y1): (i32, i32, i32, i32), x: i32, y: i32, corner: f32) -> f32 {
    let half = [(x1 - x0) as f32 / 2.0, (y1 - y0) as f32 / 2.0];
    let from = [
        x as f32 + 0.5 - (x0 + x1) as f32 / 2.0,
        y as f32 + 0.5 - (y0 + y1) as f32 / 2.0,
    ];
    let r = corner.min(half[0]).min(half[1]);
    let (qx, qy) = (from[0].abs() - half[0] + r, from[1].abs() - half[1] + r);
    let past = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r;
    (0.5 - past).clamp(0.0, 1.0)
}

fn rounded_past(x: f32, y: f32, half: [f32; 2], corner: f32) -> f32 {
    let r = [corner.min(half[0]), corner.min(half[1])];
    let (ax, ay) = (x.abs(), y.abs());
    let (qx, qy) = (ax - half[0] + r[0], ay - half[1] + r[1]);
    if qx <= 0.0 || qy <= 0.0 || r[0] <= 0.0 || r[1] <= 0.0 {
        return (ax - half[0]).max(ay - half[1]);
    }
    let k0 = (qx / r[0]).hypot(qy / r[1]);
    let k1 = (qx / (r[0] * r[0])).hypot(qy / (r[1] * r[1]));
    k0 * (k0 - 1.0) / k1
}

fn face_of(system: &FontSystem, attrs: &Attrs<'_>) -> Option<fontdb::ID> {
    system.db().query(&fontdb::Query {
        families: &[attrs.family],
        weight: attrs.weight,
        stretch: attrs.stretch,
        style: attrs.style,
    })
}

fn with_face_weight<'a>(system: &FontSystem, attrs: Attrs<'a>) -> Attrs<'a> {
    use skrifa::MetadataProvider;
    let Some(face) = face_of(system, &attrs).and_then(|id| system.db().face(id)) else {
        return attrs;
    };
    if face.weight == attrs.weight {
        return attrs;
    }
    let axis = system.db().with_face_data(face.id, |data, index| {
        let font = skrifa::FontRef::from_index(data, index).ok()?;
        let axis = font.axes().get_by_tag(skrifa::Tag::new(b"wght"))?;
        Some((axis.min_value(), axis.max_value()))
    });
    let weight = match axis.flatten() {
        Some((low, high)) => Weight(f32::from(attrs.weight.0).max(low).min(high) as u16),
        None => face.weight,
    };
    attrs.weight(weight)
}

fn font_heights(
    system: &mut FontSystem,
    id: fontdb::ID,
    weight: Weight,
    typo: bool,
) -> Option<[f64; 3]> {
    use skrifa::raw::TableProvider;
    use skrifa::raw::tables::os2::SelectionFlags;
    let index = system.db().face(id)?.index;
    let font = system.get_font(id, weight)?;
    let m = font.metrics();
    let face = skrifa::FontRef::from_index(font.data(), index).ok()?;
    let line = m.ascent - m.descent + m.leading;
    let parts = face
        .os2()
        .ok()
        .filter(|t| !typo || !t.fs_selection().contains(SelectionFlags::USE_TYPO_METRICS))
        .map_or([m.ascent, -m.descent, m.leading], |t| {
            let ascent = f32::from(t.us_win_ascent());
            let descent = f32::from(t.us_win_descent());
            [ascent, descent, (line - ascent - descent).max(0.0)]
        });
    let em = f64::from(m.units_per_em);
    Some(parts.map(|part| f64::from(part) / em))
}

fn letter_x_width(system: &mut FontSystem, attrs: &Attrs<'_>, size: f32) -> Option<f32> {
    use skrifa::MetadataProvider;
    use skrifa::outline::pen::NullPen;
    use skrifa::outline::{DrawSettings, Engine, HintingInstance, HintingOptions, Target};
    let id = face_of(system, attrs)?;
    let index = system.db().face(id)?.index;
    let font = system.get_font(id, attrs.weight)?;
    let face = skrifa::FontRef::from_index(font.data(), index).ok()?;
    let glyph = face.charmap().map('X').unwrap_or(skrifa::GlyphId::NOTDEF);
    let whole = (size + 0.5).floor();
    let at = skrifa::instance::Size::new(whole);
    let place = skrifa::instance::LocationRef::default();
    let plain = face.glyph_metrics(at, place).advance_width(glyph)?.round();
    let linear = face
        .table_data(skrifa::Tag::new(b"LTSH"))
        .and_then(|table| {
            let from = table.as_bytes().get(LTSH_HEAD + glyph.to_u32() as usize)?;
            Some(f32::from(*from))
        })
        .is_some_and(|from| whole >= from);
    if linear {
        return Some(plain);
    }
    let outlines = face.outline_glyphs();
    let options = HintingOptions {
        engine: Engine::Interpreter,
        target: Target::Mono,
    };
    let fitted = HintingInstance::new(&outlines, at, place, options)
        .ok()
        .and_then(|instance| {
            let drawn = DrawSettings::hinted(&instance, false);
            outlines
                .get(glyph)?
                .draw(drawn, &mut NullPen)
                .ok()?
                .advance_width
        });
    Some(fitted.map_or(plain, f32::round))
}

fn line_gap(system: &mut FontSystem, look: &Look, attrs: &Attrs<'_>, px: f32) -> f32 {
    look.title_gap
        .and_then(|(title, pixel)| {
            let id = face_of(system, attrs)?;
            let own = font_heights(system, id, attrs.weight, !title.moves())?;
            let zoom = f64::from(look.spacing.1) / pixel;
            Some(title.line_gap(own, f64::from(px), pixel, zoom) as f32)
        })
        .unwrap_or(px * look.line)
}

fn line_tall(system: &mut FontSystem, look: &Look, attrs: &Attrs<'_>, px: f32) -> f32 {
    look.title_gap
        .and_then(|_| {
            let id = face_of(system, attrs)?;
            let [ascent, descent, _] = font_heights(system, id, attrs.weight, true)?;
            Some(((ascent + descent) * f64::from(px)) as f32)
        })
        .unwrap_or(px * look.line)
}

fn shifts(system: &mut FontSystem, run: &LayoutRun<'_>, seat: bool) -> Vec<f32> {
    let shifted = |g: &LayoutGlyph| g.metadata & (RAISED | LOWERED) != 0;
    let whole = run.glyphs.iter().all(shifted);
    run.glyphs
        .iter()
        .map(|g| {
            let font = shifted(g)
                .then(|| system.get_font(g.font_id, g.font_weight))
                .flatten();
            let Some(font) = font else {
                return 0.0;
            };
            let m = font.metrics();
            let em = g.font_size / f32::from(m.units_per_em);
            let own = font_heights(system, g.font_id, g.font_weight, true).filter(|_| seat);
            let height = own.map_or((m.ascent - m.descent) * em, |[ascent, descent, _]| {
                (ascent + descent) as f32 * g.font_size
            });
            let lift = if g.metadata & RAISED != 0 {
                -height / 2.0
            } else {
                height / 6.0
            };
            let seated = (m.ascent + m.descent) * em / 4.0;
            if whole && !seat { lift + seated } else { lift }
        })
        .collect()
}

fn ascents(system: &mut FontSystem, buffer: &Buffer) -> Vec<f32> {
    buffer
        .layout_runs()
        .map(|r| {
            r.glyphs
                .iter()
                .filter_map(|g| {
                    let [ascent, ..] = font_heights(system, g.font_id, g.font_weight, true)?;
                    let size = if g.metadata & (RAISED | LOWERED) != 0 {
                        f32::from_bits((g.metadata >> FULL_SIZE_BIT) as u32)
                    } else {
                        g.font_size
                    };
                    Some(ascent as f32 * size)
                })
                .fold(0.0, f32::max)
        })
        .collect()
}

fn spaced<'a>(
    system: &mut FontSystem,
    buffer: &'a Buffer,
    below: &[f32],
    seat: bool,
) -> Vec<LayoutRun<'a>> {
    let count = buffer.layout_runs().count();
    let fulls = ascents(system, buffer);
    let mut shift = 0.0;
    buffer
        .layout_runs()
        .enumerate()
        .map(|(i, r)| {
            let own = below.get(r.line_i).copied().unwrap_or(0.0);
            let grown = if i + 1 == count { own.max(0.0) } else { own };
            let y = if seat {
                r.line_top + fulls[i]
            } else {
                r.line_y
            };
            let run = LayoutRun {
                line_y: y + shift,
                line_top: r.line_top + shift,
                line_height: r.line_height + grown,
                ..r
            };
            shift += own;
            run
        })
        .collect()
}

fn pair_kerning(font: &Font, first: &str, next: &str) -> f32 {
    let width = |text: &str| {
        let mut buffer = harfrust::UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        let shaped = font.shaper().shape(buffer, &[]);
        shaped
            .glyph_positions()
            .iter()
            .map(|p| p.x_advance)
            .sum::<i32>()
    };
    let units = width(&format!("{first}{next}")) - width(first) - width(next);
    units as f32 / f32::from(font.metrics().units_per_em)
}

fn free_line_ends(buffer: &mut Buffer) -> Vec<f32> {
    let mut freed = vec![0.0; buffer.lines.len()];
    for (line, freed) in buffer.lines.iter_mut().zip(&mut freed) {
        let Some((at, last)) = line.text().char_indices().last() else {
            continue;
        };
        let own = line.attrs_list().get_span(at);
        if let Some(spacing) = own.letter_spacing_opt.map(|s| s.0).filter(|s| *s < 0.0) {
            let mut list = line.attrs_list().clone();
            list.add_span(at..at + last.len_utf8(), &own.letter_spacing(0.0));
            line.set_attrs_list(list);
            *freed = -spacing;
        }
    }
    freed
}

fn kern_words(system: &mut FontSystem, buffer: &mut Buffer) -> bool {
    let mut changed = false;
    for line in &mut buffer.lines {
        let mut list = line.attrs_list().clone();
        let mut found = false;
        let words = line.shape_opt().into_iter().flat_map(|s| &s.spans);
        for span in words.filter(|s| !s.level.is_rtl()) {
            for pair in span.words.windows(2) {
                let (Some(a), Some(b)) = (pair[0].glyphs.last(), pair[1].glyphs.first()) else {
                    continue;
                };
                let own = line.attrs_list().get_span(a.start);
                if a.font_id != b.font_id || own != line.attrs_list().get_span(b.start) {
                    continue;
                }
                let Some(font) = system.get_font(a.font_id, a.font_weight) else {
                    continue;
                };
                let text = line.text();
                let kerning = pair_kerning(&font, &text[a.start..a.end], &text[b.start..b.end]);
                if kerning != 0.0 {
                    let spacing = own.letter_spacing_opt.map_or(0.0, |s| s.0) + kerning;
                    list.add_span(a.start..a.end, &own.letter_spacing(spacing));
                    found = true;
                }
            }
        }
        if found {
            line.set_attrs_list(list);
            changed = true;
        }
    }
    changed
}

fn lay_out(look: &Look) -> Option<Laid> {
    let size = look.size;
    let line = look.line;
    let wrap = look.wrap;
    let mut held = fonts().lock().unwrap_or_else(|e| e.into_inner());
    let f = &mut *held;
    f.limit_glyphs();
    let installed_family = look.font.and_then(|name| installed(f, name));
    let family = match installed_family.as_deref() {
        Some(name) => Family::Name(name),
        None => Family::SansSerif,
    };
    let base = with_face_weight(&f.system, Attrs::new().family(family));
    let gap = line_gap(&mut f.system, look, &base, size);
    let tall = line_tall(&mut f.system, look, &base, size);
    let mut buffer = Buffer::new(&mut f.system, Metrics::new(size, gap));
    buffer.set_wrap(Wrap::WordOrGlyph);
    buffer.set_size(Some(wrap), None);
    let runs = match look.markup {
        Markup::Title(text, _) => vidcut_core::rich::spans(text),
        Markup::Pieces(..) => Vec::new(),
    };
    let faces: Vec<Option<String>> = runs
        .iter()
        .map(|r| r.look.face.as_deref().and_then(|n| installed(f, n)))
        .collect();
    let word_faces: Vec<Option<String>> = match look.markup {
        Markup::Pieces(pieces) => pieces
            .iter()
            .map(|p| p.face.as_deref().and_then(|n| installed(f, n)))
            .collect(),
        Markup::Title(..) => Vec::new(),
    };
    let seat = look.seat.is_some();
    let flags = if seat {
        CacheKeyFlags::DISABLE_HINTING
    } else {
        CacheKeyFlags::empty()
    };
    let mut before = String::new();
    let spans = match look.markup {
        Markup::Pieces(pieces) => spans(
            &f.system,
            pieces,
            family,
            &word_faces,
            color(look.ink),
            size,
            line,
        ),
        Markup::Title(_, per) => runs
            .iter()
            .zip(&faces)
            .flat_map(|(r, face)| {
                let ink = r.look.color.as_deref().and_then(rgb).unwrap_or(look.ink);
                let mut a = Attrs::new()
                    .family(face.as_deref().map_or(family, Family::Name))
                    .color(color(ink))
                    .cache_key_flags(flags);
                if r.look.bold {
                    a = a.weight(Weight::BOLD);
                }
                if r.look.italic {
                    a = a.style(Style::Italic);
                }
                a = with_face_weight(&f.system, a);
                if r.look.underline {
                    a = a.underline(UnderlineStyle::Single);
                }
                if r.look.overline {
                    a = a.overline();
                }
                if r.look.strike {
                    a = a.strikethrough();
                }
                let full = r.look.size.map_or(size, |s| {
                    let exact = s * f64::from(per);
                    let drawn = if seat {
                        Title::whole_size(exact, f64::from(look.spacing.1))
                    } else {
                        exact
                    };
                    (drawn as f32).max(1.0)
                });
                let gap = line_gap(&mut f.system, look, &a, full);
                let px = match r.look.shift {
                    Some(_) => {
                        Title::shifted_size(f64::from(full), f64::from(look.spacing.1)) as f32
                    }
                    None => full,
                };
                let full_bits = (full.to_bits() as usize) << FULL_SIZE_BIT;
                a = a.metadata(match r.look.shift {
                    Some(Shift::Up) => RAISED | full_bits,
                    Some(Shift::Down) => LOWERED | full_bits,
                    None => 0,
                });
                let tracking = r.look.tracking.map_or(look.tracking, |t| t as f32);
                let pixels = if tracking == 0.0 {
                    0.0
                } else {
                    let unzoomed = full / look.spacing.1;
                    let letter = letter_x_width(&mut f.system, &a, unzoomed).unwrap_or(unzoomed);
                    Title::tracking_pixels(f64::from(tracking), f64::from(letter)) as f32
                        * look.spacing.1
                };
                let pieces = r.look.case.unwrap_or(look.case).pieces(&r.text, &before);
                before.push_str(&r.text);
                let scale = f64::from(look.spacing.1);
                pieces.into_iter().map(move |(text, small)| {
                    let px = if small {
                        Title::small_size(f64::from(px), scale) as f32
                    } else {
                        px
                    };
                    let mut b = a.clone().metrics(Metrics::new(px, gap));
                    if pixels != 0.0 {
                        b = b.letter_spacing(pixels / px);
                    }
                    (text, b)
                })
            })
            .collect(),
    };
    buffer.set_rich_text(
        spans.iter().map(|(s, a)| (s.as_str(), a.clone())),
        &base,
        Shaping::Advanced,
        Some(look.align),
    );
    buffer.shape_until_scroll(&mut f.system, false);
    if seat && kern_words(&mut f.system, &mut buffer) {
        buffer.shape_until_scroll(&mut f.system, false);
    }
    let freed = if seat {
        free_line_ends(&mut buffer)
    } else {
        Vec::new()
    };
    if freed.iter().any(|space| *space > 0.0) {
        buffer.shape_until_scroll(&mut f.system, false);
    }
    if buffer.layout_runs().map(|r| r.line_height).sum::<f32>() <= 0.0 {
        return None;
    }
    let below: Vec<f32> = match look.markup {
        Markup::Title(text, _) => vidcut_core::rich::lines(text)
            .iter()
            .map(|l| {
                vidcut_core::rich::spacing(l).map_or(look.spacing.0, |s| s as f32) * look.spacing.1
            })
            .collect(),
        Markup::Pieces(..) => Vec::new(),
    };
    let runs = spaced(&mut f.system, &buffer, &below, seat);
    let lifts: Vec<Vec<f32>> = runs
        .iter()
        .map(|r| shifts(&mut f.system, r, seat))
        .collect();
    let lines: f32 = runs.iter().map(|r| r.line_height).sum();
    let rise = runs.iter().map(|r| r.line_top).fold(0.0, f32::min);
    let inked = runs
        .iter()
        .zip(buffer.layout_runs())
        .map(|(r, plain)| r.line_top + plain.line_height)
        .fold(lines, f32::max);
    let widest = runs.iter().map(|r| r.line_w).fold(0.0_f32, f32::max);
    let last = runs.iter().map(|r| r.line_i).max();
    let above: f32 = runs
        .iter()
        .filter(|r| Some(r.line_i) != last)
        .map(|r| r.line_height)
        .sum();
    let seated = look.seat.zip(look.title_gap).map(|(s, (t, pixel))| {
        let (middle, wide, wrap) = (s.middle, widest, look.wrap);
        let left = t.seated_left(
            s.anchor[0],
            f64::from(middle),
            f64::from(wide),
            f64::from(wrap),
            pixel,
        );
        let edge = ((f64::from(s.edge) / pixel).round() * pixel) as f32;
        let top = block_top(edge, s.part, s.anchor[1] as f32, lines, above);
        [left as f32, top.ceil()]
    });
    let exact = seated.map(|at| at.map(|v| v - v.round()));
    let popped = runs.iter().find_map(|run| {
        let (x0, x1) = run
            .glyphs
            .iter()
            .filter(|g| g.metadata & POPPED != 0)
            .fold((f32::MAX, f32::MIN), |(a, b), g| {
                (a.min(g.x), b.max(g.x + g.w))
            });
        (x1 > x0).then(|| ((x0 + x1) / 2.0, run.line_top + run.line_height / 2.0))
    });
    let stroke = look.stroke;
    let backing = look.backing;
    let outline = stroke.map_or(0.0, |s| s.past());
    let frame = look.frame;
    let reach = look.shadow.map_or(0, |s| {
        s.shift.0.abs().max(s.shift.1.abs()) + (3.0 * s.sigma).ceil() as i32 + 1
    });
    let pad = if backing.is_some() {
        (outline.ceil() as i32 + 2).max(frame.down.ceil() as i32)
    } else {
        outline.ceil() as i32 + 2
    } + reach
        + (-rise).ceil() as i32;
    let band = backing
        .and_then(|_| {
            runs.iter()
                .filter_map(|run| {
                    let (x0, x1) = run.glyphs.iter().fold((f32::MAX, f32::MIN), |(a, b), g| {
                        (a.min(g.x), b.max(g.x + g.w))
                    });
                    (x1 > x0).then_some((x0, run.line_top, x1, run.line_top + run.line_height))
                })
                .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
        })
        .map(|(x0, y0, x1, y1)| {
            let dropped = if above > 0.0 { look.drop } else { 0.0 };
            (
                (x0 - frame.across).floor() as i32,
                (y0 + dropped - frame.down).floor() as i32,
                (x1 + frame.across).ceil() as i32,
                (y1 + frame.down).ceil() as i32,
            )
        });
    let (mut gx0, mut gx1) = (i32::MAX, i32::MIN);
    for run in &runs {
        for g in run.glyphs {
            gx0 = gx0.min(g.x.floor() as i32);
            gx1 = gx1.max((g.x + g.w).ceil() as i32);
        }
    }
    if gx1 <= gx0 {
        return None;
    }
    let margin = (size * 0.5).ceil() as i32 + pad;
    let left = gx0 - margin;
    let (cw, ch) = (gx1 - gx0 + 2 * margin, inked.ceil() as i32 + 2 * pad);
    let (w, h) = (cw as usize, ch as usize);
    let mut canvas = Canvas {
        pixels: vec![[0.0f32; 4]; w * h],
        width: cw,
        height: ch,
        left,
        top: -pad,
    };
    let white = Color::rgb(255, 255, 255);
    let drawn_lines: Option<Vec<Vec<TextLine>>> = look.title_gap.map(|_| {
        let nudge = exact.map_or(0.0, |at| at[0]);
        let origin = runs
            .iter()
            .flat_map(|run| run.glyphs)
            .fold(f32::MAX, |x, g| x.min(g.x));
        let place = (look.spacing.1, origin, nudge, rise);
        runs.iter()
            .zip(&lifts)
            .map(|(run, lift)| text_lines(&mut f.system, run, (lift, &freed), place, white))
            .collect()
    });
    if let Some(at) = exact.filter(|_| !look.bare) {
        letters(&mut canvas, &runs, &lifts, at, white, f);
    }
    let mut painter = cosmic_text::LegacyRenderer {
        font_system: &mut f.system,
        cache: &mut f.cache,
        callback: |x: i32, y: i32, bw: u32, bh: u32, color: Color| {
            canvas.rectangle(x, y, bw, bh, color);
        },
    };
    for (i, (run, lift)) in runs.iter().zip(&lifts).enumerate().filter(|_| !look.bare) {
        for (g, up) in run
            .glyphs
            .iter()
            .zip(lift)
            .filter(|(g, _)| exact.is_none() && g.metadata & HIDDEN == 0)
        {
            painter.glyph(
                g.physical((0., run.line_y + up), 1.0),
                g.color_opt.unwrap_or(white),
            );
        }
        let own = drawn_lines.as_ref().map(|all| all[i].as_slice());
        decorate(&mut painter, run, white, own);
    }
    let traced = stroke.and_then(|_| traces(&runs, &lifts, exact, drawn_lines.as_deref(), f));
    drop(held);
    let canvas = canvas.pixels;
    let alpha: Vec<f32> = canvas.iter().map(|p| p[3]).collect();
    let on_top = traced.is_some() && stroke.is_some_and(|s| !s.outside);
    let (ring, edge) = match (stroke, traced) {
        (Some(s), Some(list)) if s.outside => {
            let mut ring = alpha.clone();
            stroked(&list, s.width, &mut ring, w, (left, -pad));
            (ring, s.ink)
        }
        (Some(s), Some(list)) => {
            let mut ring = vec![0.0; alpha.len()];
            stroked(&list, s.width, &mut ring, w, (left, -pad));
            (ring, s.ink)
        }
        (Some(s), None) => (dilate(&alpha, w, h, s.past()), s.ink),
        (None, _) => (vec![0.0; alpha.len()], [0, 0, 0]),
    };
    let shade: Vec<f32> = match look.shadow {
        Some(s) => {
            let shape: Vec<f32> = alpha
                .iter()
                .zip(&ring)
                .map(|(&a, &o)| a + o * (1.0 - a))
                .collect();
            let gain = TitleShadow::GAIN as f32;
            soften(&shifted(&shape, w, h, s.shift), w, h, s.sigma)
                .into_iter()
                .map(|v| (v * gain).min(1.0) * s.opacity)
                .collect()
        }
        None => Vec::new(),
    };
    let float = look.float && !look.bare && (look.shadow.is_some() || band.is_some());
    let rgba: Vec<[f32; 4]> = canvas
        .iter()
        .zip(&ring)
        .enumerate()
        .map(|(i, (p, &o))| {
            let (x, y) = ((i % w) as i32 + left, (i / w) as i32 - pad);
            let bg = match (backing, band) {
                (Some((c, a)), Some(band)) => unit(c, a as f32 * covered(band, x, y, frame.corner)),
                _ => [0.0; 4],
            };
            let text = if p[3] > 0.0 {
                [p[0] / p[3], p[1] / p[3], p[2] / p[3], p[3]]
            } else {
                [0.0; 4]
            };
            let shadow = look.shadow.map(|sh| unit(sh.ink, shade[i]));
            if float {
                let letters = if on_top {
                    over(text, unit(edge, o))
                } else {
                    over(unit(edge, o), text)
                };
                let under = stepped(bg);
                let under = shadow.map_or(under, |s| over(under, s));
                return over(under, stepped(letters));
            }
            let behind = match shadow {
                Some(s) => over(bg, s),
                None => bg,
            };
            if on_top {
                over(over(behind, text), unit(edge, o))
            } else {
                over(over(behind, unit(edge, o)), text)
            }
        })
        .collect();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (i, p) in rgba.iter().enumerate() {
        if p[3] > 0.0 {
            let (x, y) = (i % w, i / w);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    let sprite = (x1 > x0 && y1 > y0).then(|| Sprite {
        x: x0 as i32 + left,
        y: y0 as i32 - pad,
        width: (x1 - x0) as u32,
        height: (y1 - y0) as u32,
        lift: 0.0,
        float,
        rgba: (y0..y1)
            .flat_map(|y| rgba[y * w + x0..y * w + x1].iter().copied())
            .collect(),
    });
    Some(Laid {
        sprite,
        lines,
        above,
        tall,
        popped,
        seated,
    })
}

fn inside(s: Sprite, (w, h): (f64, f64)) -> Option<Sprite> {
    let x0 = s.x.max(0);
    let y0 = s.y.max(0);
    let x1 = (s.x + s.width as i32).min(w as i32);
    let y1 = (s.y + s.height as i32).min(h as i32);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let mut rgba = Vec::with_capacity((x1 - x0) as usize * (y1 - y0) as usize);
    for y in y0..y1 {
        for x in x0..x1 {
            rgba.push(s.at(x, y));
        }
    }
    Some(Sprite {
        x: x0,
        y: y0,
        width: (x1 - x0) as u32,
        height: (y1 - y0) as u32,
        lift: s.lift,
        float: s.float,
        rgba,
    })
}

fn block_top(edge: f32, part: TitlePart, down: f32, lines: f32, above: f32) -> f32 {
    edge - match part {
        TitlePart::Whole | TitlePart::Upper | TitlePart::Lower | TitlePart::Shape => down * lines,
        TitlePart::Backing => above + down * (lines - above),
    }
}

fn placed(mut sprite: Sprite, left: f32, top: f32) -> Sprite {
    sprite.x += left.round() as i32;
    sprite.y += top.round() as i32;
    sprite
}

fn block(
    pieces: &[Piece],
    pop: f64,
    place: Place,
    (width, height): (u32, u32),
    timeline: u32,
    style: &SubtitleStyle,
) -> (Option<Sprite>, Option<Sprite>) {
    let (w, h) = (f64::from(width), f64::from(height));
    let size = (height as f32 * style.text_fraction() as f32).max(1.0);
    let pixel = h / f64::from(timeline.max(1));
    let look = Look {
        font: style.font.as_deref(),
        size,
        line: SubtitleStyle::LINE as f32,
        title_gap: None,
        seat: None,
        spacing: (0.0, 0.0),
        tracking: 0.0,
        case: TitleCase::Mixed,
        wrap: (style.wrap_width(w, h) as f32).max(size),
        align: text_align(style),
        ink: style.ink(),
        stroke: style.stroke().map(|s| Edge::of(s, pixel)),
        backing: style.backing(),
        frame: Frame {
            across: (style.text_fraction() * SubtitleStyle::BOX_ACROSS * pixel) as f32,
            down: (SubtitleStyle::BOX_DOWN * pixel) as f32,
            corner: (h * SubtitleStyle::BOX_CORNER) as f32,
        },
        shadow: style.shadow.as_ref().and_then(|s| shade(s, w, h)),
        markup: Markup::Pieces(pieces),
        bare: false,
        drop: 0.0,
        float: false,
    };
    let line = Line {
        pieces: pieces.to_vec(),
        place,
        size: (width, height),
        timeline,
        style: style.clone(),
    };
    let Some(laid) = laid_line(line, &look) else {
        return (None, None);
    };
    let (left, top) = style.block_box(place, (w, h), f64::from(look.wrap), f64::from(laid.lines));
    let (left, top) = (left as f32, top as f32);
    let drawn = laid.sprite.map(|s| placed(s, left, top));
    let Some((cx, cy)) = laid.popped else {
        return (drawn, None);
    };
    {
        let word: Vec<Piece> = pieces
            .iter()
            .filter(|p| p.popped)
            .map(|p| Piece {
                popped: false,
                hidden: false,
                ..p.clone()
            })
            .collect();
        let big = Look {
            size: size * pop as f32,
            wrap: width as f32 * 4.0,
            stroke: look.stroke.map(|s| Edge {
                width: s.width * pop as f32,
                ..s
            }),
            backing: None,
            markup: Markup::Pieces(&word),
            ..look
        };
        let popped = lay_out(&big).and_then(|l| {
            Some(placed(
                l.sprite?,
                left + cx - big.wrap / 2.0,
                top + cy - l.lines / 2.0,
            ))
        });
        (drawn, popped)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Drawing {
    pub lines: Vec<Sprite>,
    pub pops: Vec<Sprite>,
}

impl Drawing {
    pub fn all(self) -> Vec<Sprite> {
        self.lines.into_iter().chain(self.pops).collect()
    }
}

fn text_align(style: &SubtitleStyle) -> Align {
    match style.line_align() {
        SubtitleAlign::Left => Align::Left,
        SubtitleAlign::Center => Align::Center,
        SubtitleAlign::Right => Align::Right,
    }
}

pub struct Painted<'a> {
    pub shown: &'a Shown,
    pub place: Place,
    pub style: &'a SubtitleStyle,
}

pub fn paint(cues: &[Painted<'_>], width: u32, height: u32, timeline: u32) -> Drawing {
    let mut drawing = Drawing::default();
    for place in [Place::Top, Place::Middle, Place::Bottom] {
        let here: Vec<&Painted<'_>> = cues
            .iter()
            .filter(|c| c.place == place && !c.shown.pieces.is_empty())
            .collect();
        let mut i = 0;
        while i < here.len() {
            let style = here[i].style;
            let mut j = i + 1;
            while j < here.len() && here[j].style == style {
                j += 1;
            }
            let mut pieces: Vec<Piece> = Vec::new();
            for (k, c) in here[i..j].iter().enumerate() {
                if k > 0 {
                    pieces.push(Piece {
                        text: "\n".into(),
                        ..Piece::default()
                    });
                }
                pieces.extend(c.shown.pieces.iter().cloned());
            }
            let pop = here[i..j].iter().map(|c| c.shown.pop).fold(1.0, f64::max);
            let (line, popped) = match pieces.is_empty() {
                true => (None, None),
                false => block(&pieces, pop, place, (width, height), timeline, style),
            };
            drawing.lines.extend(line);
            drawing.pops.extend(popped);
            i = j;
        }
    }
    drawing
}

pub fn draw(cues: &[(&Shown, Place)], width: u32, height: u32, style: &SubtitleStyle) -> Drawing {
    let painted: Vec<Painted<'_>> = cues
        .iter()
        .map(|(shown, place)| Painted {
            shown,
            place: *place,
            style,
        })
        .collect();
    paint(&painted, width, height, height)
}

pub fn title(t: &Title, width: u32, height: u32, timeline: u32) -> Vec<Sprite> {
    let blocks = title_blocks(t, width, height, timeline);
    let parts = t.blocks();
    let kept = blocks
        .into_iter()
        .zip(parts)
        .filter(|(_, b)| !(t.moves() && b.part == TitlePart::Shape))
        .filter_map(|(s, _)| s);
    merged(kept.collect()).into_iter().collect()
}

pub fn title_blocks(t: &Title, width: u32, height: u32, timeline: u32) -> Vec<Option<Sprite>> {
    if t.text.trim().is_empty() {
        return Vec::new();
    }
    t.blocks()
        .iter()
        .map(|b| title_block(t, b, width, height, timeline))
        .collect()
}

fn merged(mut sprites: Vec<Sprite>) -> Option<Sprite> {
    if sprites.len() < 2 {
        return sprites.pop();
    }
    let x0 = sprites.iter().map(|s| s.x).min()?;
    let y0 = sprites.iter().map(|s| s.y).min()?;
    let x1 = sprites.iter().map(|s| s.x + s.width as i32).max()?;
    let y1 = sprites.iter().map(|s| s.y + s.height as i32).max()?;
    let float = sprites.iter().any(|s| s.float);
    let at = |s: &Sprite, x: i32, y: i32| match float && !s.float {
        true => stepped(s.at(x, y)),
        false => s.at(x, y),
    };
    let mut rgba = Vec::with_capacity((x1 - x0) as usize * (y1 - y0) as usize);
    for y in y0..y1 {
        for x in x0..x1 {
            rgba.push(sprites.iter().fold([0.0; 4], |a, s| over(a, at(s, x, y))));
        }
    }
    Some(Sprite {
        x: x0,
        y: y0,
        width: (x1 - x0) as u32,
        height: (y1 - y0) as u32,
        lift: 0.0,
        float,
        rgba,
    })
}

fn shaped_box(t: &Title, block: &TitleBlock, (w, h): (f64, f64), pixel: f64) -> Option<Sprite> {
    let shape = t.shape()?;
    let (fill, opacity) = t.backing()?;
    let (ring, ring_width) = t.shape_outline();
    let [left, top, across, down] = t.shape_pixels(block.pivot, (w, h))?;
    let half = [across / 2.0, down / 2.0].map(|v| v as f32);
    if half[0] <= 0.0 || half[1] <= 0.0 {
        return None;
    }
    let middle = [left + across / 2.0, top + down / 2.0].map(|v| v as f32);
    let corner = (shape.corner.unwrap_or(Title::BOX_CORNER) * h) as f32;
    let line = (ring_width * pixel) as f32;
    let reach = line / 2.0 + 1.0;
    let x0 = ((middle[0] - half[0] - reach).floor() as i32).max(0);
    let x1 = ((middle[0] + half[0] + reach).ceil() as i32).min(w as i32);
    let y0 = ((middle[1] - half[1] - reach).floor() as i32).max(0);
    let y1 = ((middle[1] + half[1] + reach).ceil() as i32).min(h as i32);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (fill, ring) = (unit(fill, 1.0), unit(ring, 1.0));
    let mut rgba = Vec::with_capacity((x1 - x0) as usize * (y1 - y0) as usize);
    for y in y0..y1 {
        for x in x0..x1 {
            let past = rounded_past(
                x as f32 + 0.5 - middle[0],
                y as f32 + 0.5 - middle[1],
                half,
                corner,
            );
            let inside = (0.5 - past).clamp(0.0, 1.0) * opacity as f32;
            let edge = if line > 0.0 {
                (line / 2.0 + 0.5 - past.abs()).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let under = inside * (1.0 - edge);
            let a = edge + under;
            rgba.push(if a > 0.0 {
                [
                    (ring[0] * edge + fill[0] * under) / a,
                    (ring[1] * edge + fill[1] * under) / a,
                    (ring[2] * edge + fill[2] * under) / a,
                    a,
                ]
            } else {
                [0.0; 4]
            });
        }
    }
    let sprite = Sprite {
        x: x0,
        y: y0,
        width: (x1 - x0) as u32,
        height: (y1 - y0) as u32,
        lift: 0.0,
        float: false,
        rgba,
    };
    let [px, py] = block.pivot;
    Some(turned(
        sprite,
        ((px * w) as f32, (py * h) as f32),
        block.turn,
    ))
}

fn title_block(
    t: &Title,
    block: &TitleBlock,
    width: u32,
    height: u32,
    timeline: u32,
) -> Option<Sprite> {
    let (w, h) = (f64::from(width), f64::from(height));
    if block.part == TitlePart::Shape {
        return shaped_box(t, block, (w, h), h / f64::from(timeline.max(1)));
    }
    let zoom = block.zoom;
    let pixel = h / f64::from(timeline.max(1));
    let exact = h * t.text_fraction() * zoom;
    let drawn = match t.kind {
        TitleKind::Scroll => exact,
        TitleKind::Text | TitleKind::LowerThird => Title::whole_size(exact, pixel * zoom),
    };
    let size = drawn.max(1.0) as f32;
    let slack = (size * Title::PADDING as f32).ceil();
    let text = t.block_text(block.part);
    let bare = block.part == TitlePart::Backing;
    let boxed = matches!(block.part, TitlePart::Whole | TitlePart::Backing);
    let look = Look {
        font: t.style.font.as_deref(),
        size,
        line: Title::LINE as f32,
        title_gap: Some((t, pixel)),
        seat: (t.kind != TitleKind::Scroll).then_some(Seat {
            middle: (block.pivot[0] * w) as f32,
            edge: (block.edge * h) as f32,
            part: block.part,
            anchor: block.anchor,
        }),
        spacing: (
            t.style.line_spacing.unwrap_or(0.0) as f32,
            (pixel * zoom) as f32,
        ),
        tracking: t.style.tracking.unwrap_or(0.0) as f32,
        case: t.style.case.unwrap_or_default(),
        wrap: ((t.wrap_width(w, h) * zoom) as f32).max(size),
        align: match t.align() {
            TitleAlign::Left => Align::Left,
            TitleAlign::Middle => Align::Center,
            TitleAlign::Right => Align::Right,
        },
        ink: t.ink(),
        stroke: t
            .stroke()
            .filter(|_| !bare)
            .map(|s| Edge::of(s, pixel * zoom)),
        backing: t.backing().filter(|_| boxed && t.shape().is_none()),
        frame: Frame {
            across: slack,
            down: slack,
            corner: 0.0,
        },
        shadow: t
            .style
            .shadow
            .as_ref()
            .filter(|_| !bare)
            .and_then(|s| shade(s, w, h)),
        markup: Markup::Title(&text, (h * zoom) as f32),
        bare,
        drop: (h * Title::UPPER_DROP) as f32,
        float: true,
    };
    let Laid {
        sprite: Some(sprite),
        lines,
        above,
        seated,
        tall,
        ..
    } = lay_out(&look)?
    else {
        return None;
    };
    let [px, py] = block.pivot;
    let [across, down] = block.anchor;
    let lined = t.align().fraction() as f32;
    let exact = block_top(
        (block.edge * h) as f32,
        block.part,
        down as f32,
        lines,
        above,
    );
    let [left, top] =
        seated.unwrap_or([((px + 0.5 - across) * w) as f32 - lined * look.wrap, exact]);
    let sprite = Sprite {
        lift: if t.moves() {
            (0.5 - down as f32) * tall
        } else {
            0.0
        },
        ..placed(sprite, left, top)
    };
    if block.turn == 0.0 {
        return Some(sprite);
    }
    Some(turned(
        inside(sprite, (w, h))?,
        ((px * w) as f32, (py * h) as f32),
        block.turn,
    ))
}

fn turned(s: Sprite, (px, py): (f32, f32), degrees: f64) -> Sprite {
    if degrees == 0.0 || s.width == 0 || s.height == 0 {
        return s;
    }
    let (sin, cos) = (
        degrees.to_radians().sin() as f32,
        degrees.to_radians().cos() as f32,
    );
    let forward = |x: f32, y: f32| {
        let (u, v) = (x - px, y - py);
        (px + u * cos + v * sin, py - u * sin + v * cos)
    };
    let (x0, y0) = (s.x as f32, s.y as f32);
    let (x1, y1) = (x0 + s.width as f32, y0 + s.height as f32);
    let corners = [
        forward(x0, y0),
        forward(x1, y0),
        forward(x0, y1),
        forward(x1, y1),
    ];
    let lo_x = corners.iter().map(|c| c.0).fold(f32::MAX, f32::min).floor() as i32;
    let hi_x = corners.iter().map(|c| c.0).fold(f32::MIN, f32::max).ceil() as i32;
    let lo_y = corners.iter().map(|c| c.1).fold(f32::MAX, f32::min).floor() as i32;
    let hi_y = corners.iter().map(|c| c.1).fold(f32::MIN, f32::max).ceil() as i32;
    let (ow, oh) = ((hi_x - lo_x).max(1) as usize, (hi_y - lo_y).max(1) as usize);
    let held = |x: i32, y: i32| -> [f32; 4] {
        let p = s.at(x, y);
        [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]]
    };
    let mut rgba = vec![[0.0f32; 4]; ow * oh];
    for oy in 0..oh {
        for ox in 0..ow {
            let (u, v) = (
                lo_x as f32 + ox as f32 + 0.5 - px,
                lo_y as f32 + oy as f32 + 0.5 - py,
            );
            let (sx, sy) = (px + u * cos - v * sin - 0.5, py + u * sin + v * cos - 0.5);
            let (fx, fy) = (sx.floor(), sy.floor());
            let (ax, ay) = (sx - fx, sy - fy);
            let (ix, iy) = (fx as i32, fy as i32);
            let mut p = [0.0f32; 4];
            for (dx, dy, k) in [
                (0, 0, (1.0 - ax) * (1.0 - ay)),
                (1, 0, ax * (1.0 - ay)),
                (0, 1, (1.0 - ax) * ay),
                (1, 1, ax * ay),
            ] {
                let q = held(ix + dx, iy + dy);
                for c in 0..4 {
                    p[c] += q[c] * k;
                }
            }
            rgba[oy * ow + ox] = if p[3] > 0.0 {
                [p[0] / p[3], p[1] / p[3], p[2] / p[3], p[3]]
            } else {
                [0.0; 4]
            };
        }
    }
    Sprite {
        x: lo_x,
        y: lo_y,
        width: ow as u32,
        height: oh as u32,
        lift: s.lift,
        float: s.float,
        rgba,
    }
}

pub fn backing_box(t: &Title, width: u32, height: u32) -> Option<[f64; 4]> {
    t.backing()?;
    if t.shape().is_some() {
        return None;
    }
    let upright = Title {
        rotation: None,
        upper: None,
        style: TitleStyle {
            shadow: None,
            ..t.style.clone()
        },
        ..t.clone()
    };
    let s = title(&upright, width, height, height).into_iter().next()?;
    let (w, h) = (f64::from(width), f64::from(height));
    Some([
        f64::from(s.x) / w,
        f64::from(s.y) / h,
        f64::from(s.width) / w,
        f64::from(s.height) / h,
    ])
}

impl Sprite {
    pub fn premultiplied(&self) -> Vec<u8> {
        if !self.float {
            return self.rgba.iter().flat_map(|p| eight_bit(*p)).collect();
        }
        self.rgba
            .iter()
            .flat_map(|p| {
                let a = p[3].clamp(0.0, 1.0);
                [p[0] * a, p[1] * a, p[2] * a, a].map(|v| v.clamp(0.0, 1.0))
            })
            .flat_map(f32::to_le_bytes)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{Drawing, draw};
    use vidcut_core::model::{Place, SubtitleBackground, SubtitleStyle};
    use vidcut_core::subtitles::{Piece, Shown};

    fn drawn(piece: Piece) -> Drawing {
        let shown = Shown {
            pieces: vec![piece],
            spoken: Some(0),
            step: Some(0),
            pop: 1.15,
        };
        draw(
            &[(&shown, Place::Bottom)],
            1280,
            720,
            &SubtitleStyle {
                background: Some(SubtitleBackground {
                    color: "#000000".into(),
                    opacity: Some(0.0),
                    off: None,
                }),
                ..SubtitleStyle::default()
            },
        )
    }

    #[test]
    fn a_hidden_word_leaves_no_ink() {
        let d = drawn(Piece {
            text: "Hello".into(),
            hidden: true,
            ..Piece::default()
        });
        assert_eq!(d, Drawing::default());
    }

    #[test]
    fn a_word_color_changes_its_ink() {
        let plain = drawn(Piece {
            text: "Hi".into(),
            ..Piece::default()
        });
        let red = drawn(Piece {
            text: "Hi".into(),
            color: Some("#ff0000".into()),
            ..Piece::default()
        });
        assert_ne!(plain.lines, red.lines);
    }

    #[test]
    fn a_lone_popping_word_is_drawn() {
        let d = drawn(Piece {
            text: "Hello".into(),
            popped: true,
            ..Piece::default()
        });
        assert_eq!((d.lines.len(), d.pops.len()), (0, 1));
    }
}
