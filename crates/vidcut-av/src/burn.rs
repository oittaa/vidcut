use std::sync::{Mutex, MutexGuard, OnceLock};

use cosmic_text::{
    Align, Attrs, Buffer, Color, Family, FontSystem, Metrics, Renderer, Shaping, Style, SwashCache,
    UnderlineStyle, Weight, Wrap,
};
use vidcut_core::lru::Lru;
use vidcut_core::model::{
    Place, SubtitleAlign, SubtitleStyle, Title, TitleAlign, TitleKind, TitleShadow, TitleStyle,
};
use vidcut_core::subtitles::{Piece, Shown, rgb};

use crate::error::{Error, Result};
use crate::ffmpeg::{ffi, format, frame};

const FAMILIES: &[&str] = &[
    "Open Sans",
    "Segoe UI",
    "Helvetica Neue",
    "Noto Sans",
    "DejaVu Sans",
    "Arial",
    "Liberation Sans",
];

pub(crate) type Placing = ((i32, i32), f32);

#[derive(Debug, Clone, PartialEq)]
pub struct Sprite {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
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

struct Fonts {
    system: FontSystem,
    cache: SwashCache,
}

fn installed_usual(system: &FontSystem) -> Option<&'static str> {
    FAMILIES.iter().copied().find(|name| {
        system
            .db()
            .faces()
            .any(|f| f.families.iter().any(|(n, _)| n == name))
    })
}

fn fonts() -> &'static Mutex<Fonts> {
    static FONTS: OnceLock<Mutex<Fonts>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut system = FontSystem::new();
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

fn spans<'a>(
    pieces: &[Piece],
    family: Family<'a>,
    faces: &'a [Option<String>],
    ink: Color,
    bold: bool,
    size: f32,
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
                a = a.metrics(Metrics::new(px, px * SubtitleStyle::LINE as f32));
            }
            if p.italic {
                a = a.style(Style::Italic);
            }
            if p.bold || bold {
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
            (p.text.clone(), a)
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

struct Look<'a> {
    font: Option<&'a str>,
    size: f32,
    wrap: f32,
    align: Align,
    ink: [u8; 3],
    stroke: Option<([u8; 3], f64)>,
    backing: Option<([u8; 3], f64)>,
    shadow: Option<Shade>,
    boxed: bool,
    markup: Markup<'a>,
}

#[derive(Debug, Clone, Copy)]
enum Markup<'a> {
    Pieces(&'a [Piece], bool),
    Title(&'a str, f32),
}

#[derive(Clone)]
struct Laid {
    sprite: Option<Sprite>,
    lines: f32,
    popped: Option<(f32, f32)>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Line {
    pieces: Vec<Piece>,
    place: Place,
    size: (u32, u32),
    style: SubtitleStyle,
}

const LINES: usize = 8;

fn lines() -> MutexGuard<'static, Lru<Line, Laid, LINES>> {
    static LINES_KEPT: Mutex<Lru<Line, Laid, LINES>> = Mutex::new(Lru::new());
    LINES_KEPT.lock().unwrap_or_else(|e| e.into_inner())
}

fn laid_line(line: Line, look: &Look) -> Option<Laid> {
    if let Some(laid) = lines().get(&line) {
        return Some(laid.clone());
    }
    let laid = lay_out(look)?;
    lines().insert(line, laid.clone());
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

fn shade(s: &TitleShadow, w: f64, h: f64) -> Shade {
    let [dx, dy] = s.shift();
    Shade {
        ink: s.ink(),
        opacity: s.alpha() as f32,
        shift: ((dx * w).round() as i32, (dy * h).round() as i32),
        sigma: (s.softness() * h / TitleShadow::BLUR_PER_PICTURE) as f32,
    }
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

fn traces(buffer: &Buffer, f: &mut Fonts) -> Option<Vec<Traced>> {
    let mut bars = Bars(Vec::new());
    for run in buffer.layout_runs() {
        for g in run.glyphs.iter().filter(|g| g.metadata & HIDDEN == 0) {
            let p = g.physical((0., run.line_y), 1.0);
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
        cosmic_text::render_decoration(&mut bars, &run, Color::rgb(255, 255, 255));
    }
    Some(bars.0)
}

fn stroked(traces: &[Traced], width: f32, into: &mut [f32], w: usize, (left, top): (i32, i32)) {
    use zeno::{Cap, Format, Join, Mask, Origin, Stroke};
    let h = into.len() / w;
    for t in traces {
        let (mask, at) = Mask::new(&t.path[..])
            .format(Format::Alpha)
            .origin(Origin::BottomLeft)
            .style(Stroke::new(width).join(Join::Round).cap(Cap::Round))
            .offset(t.offset)
            .render_offset(t.offset)
            .inspect(|_, _, _| ())
            .render();
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

fn lay_out(look: &Look) -> Option<Laid> {
    let size = look.size;
    let wrap = look.wrap;
    let mut held = fonts().lock().unwrap_or_else(|e| e.into_inner());
    let f = &mut *held;
    let mut buffer = Buffer::new(
        &mut f.system,
        Metrics::new(size, size * SubtitleStyle::LINE as f32),
    );
    buffer.set_wrap(Wrap::WordOrGlyph);
    buffer.set_size(Some(wrap), None);
    let installed_family = look.font.and_then(|name| installed(f, name));
    let family = match installed_family.as_deref() {
        Some(name) => Family::Name(name),
        None => Family::SansSerif,
    };
    let runs = match look.markup {
        Markup::Title(text, _) => vidcut_core::rich::spans(text),
        Markup::Pieces(..) => Vec::new(),
    };
    let faces: Vec<Option<String>> = runs
        .iter()
        .map(|r| r.look.face.as_deref().and_then(|n| installed(f, n)))
        .collect();
    let word_faces: Vec<Option<String>> = match look.markup {
        Markup::Pieces(pieces, _) => pieces
            .iter()
            .map(|p| p.face.as_deref().and_then(|n| installed(f, n)))
            .collect(),
        Markup::Title(..) => Vec::new(),
    };
    let spans = match look.markup {
        Markup::Pieces(pieces, bold) => {
            spans(pieces, family, &word_faces, color(look.ink), bold, size)
        }
        Markup::Title(_, per) => runs
            .iter()
            .zip(&faces)
            .map(|(r, face)| {
                let ink = r.look.color.as_deref().and_then(rgb).unwrap_or(look.ink);
                let mut a = Attrs::new()
                    .family(face.as_deref().map_or(family, Family::Name))
                    .color(color(ink));
                if r.look.bold {
                    a = a.weight(Weight::BOLD);
                }
                if r.look.italic {
                    a = a.style(Style::Italic);
                }
                if r.look.underline {
                    a = a.underline(UnderlineStyle::Single);
                }
                let px = r.look.size.map_or(size, |s| (s as f32 * per).max(1.0));
                a = a.metrics(Metrics::new(px, px * SubtitleStyle::LINE as f32));
                (r.text.clone(), a)
            })
            .collect(),
    };
    let base = Attrs::new().family(family);
    buffer.set_rich_text(
        spans.iter().map(|(s, a)| (s.as_str(), a.clone())),
        &base,
        Shaping::Advanced,
        Some(look.align),
    );
    buffer.shape_until_scroll(&mut f.system, false);
    let lines: f32 = buffer.layout_runs().map(|r| r.line_height).sum();
    if lines <= 0.0 {
        return None;
    }
    let popped = buffer.layout_runs().find_map(|run| {
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
    let outline = stroke.map_or(0.0, |(_, s)| size * s as f32);
    let slack = (size * SubtitleStyle::PADDING as f32).ceil() as i32;
    let reach = look.shadow.map_or(0, |s| {
        s.shift.0.abs().max(s.shift.1.abs()) + (3.0 * s.sigma).ceil() as i32 + 1
    });
    let pad = if backing.is_some() {
        (outline.ceil() as i32 + 2).max(slack)
    } else {
        outline.ceil() as i32 + 2
    } + reach;
    let mut bands: Vec<(i32, i32, i32, i32)> = if backing.is_some() {
        buffer
            .layout_runs()
            .filter_map(|run| {
                let mut x0 = i32::MAX;
                let mut x1 = i32::MIN;
                for g in run.glyphs {
                    x0 = x0.min(g.x.floor() as i32 - slack);
                    x1 = x1.max((g.x + g.w).ceil() as i32 + slack);
                }
                (x1 > x0).then(|| {
                    (
                        x0,
                        run.line_top.floor() as i32 - slack,
                        x1,
                        (run.line_top + run.line_height).ceil() as i32 + slack,
                    )
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    if look.boxed
        && let Some(all) = bands
            .iter()
            .copied()
            .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
    {
        bands = vec![all];
    }
    let (mut gx0, mut gx1) = (i32::MAX, i32::MIN);
    for run in buffer.layout_runs() {
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
    let (cw, ch) = (gx1 - gx0 + 2 * margin, lines.ceil() as i32 + 2 * pad);
    let (w, h) = (cw as usize, ch as usize);
    let mut canvas = vec![[0.0f32; 4]; w * h];
    let white = Color::rgb(255, 255, 255);
    let mut painter = cosmic_text::LegacyRenderer {
        font_system: &mut f.system,
        cache: &mut f.cache,
        callback: |x: i32, y: i32, bw: u32, bh: u32, color: Color| {
            let a = f32::from(color.a()) / 255.0;
            if a <= 0.0 {
                return;
            }
            let rgb = [color.r(), color.g(), color.b()].map(|c| f32::from(c) / 255.0);
            for py in y..y + bh as i32 {
                for px in x..x + bw as i32 {
                    let (cx, cy) = (px - left, py + pad);
                    if cx < 0 || cy < 0 || cx >= cw || cy >= ch {
                        continue;
                    }
                    let d = &mut canvas[cy as usize * w + cx as usize];
                    for i in 0..3 {
                        d[i] = rgb[i] * a + d[i] * (1.0 - a);
                    }
                    d[3] = a + d[3] * (1.0 - a);
                }
            }
        },
    };
    for run in buffer.layout_runs() {
        for g in run.glyphs.iter().filter(|g| g.metadata & HIDDEN == 0) {
            painter.glyph(
                g.physical((0., run.line_y), 1.0),
                g.color_opt.unwrap_or(white),
            );
        }
        cosmic_text::render_decoration(&mut painter, &run, white);
    }
    let traced = stroke.and_then(|_| traces(&buffer, f));
    drop(held);
    let alpha: Vec<f32> = canvas.iter().map(|p| p[3]).collect();
    let (ring, edge) = match (stroke, traced) {
        (Some((rgb, _)), Some(list)) => {
            let mut ring = alpha.clone();
            stroked(&list, 2.0 * outline, &mut ring, w, (left, -pad));
            (ring, rgb)
        }
        (Some((rgb, _)), None) => (dilate(&alpha, w, h, outline), rgb),
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
    let rgba: Vec<[f32; 4]> = canvas
        .iter()
        .zip(&ring)
        .enumerate()
        .map(|(i, (p, &o))| {
            let (x, y) = ((i % w) as i32 + left, (i / w) as i32 - pad);
            let bg = backing
                .filter(|_| {
                    bands
                        .iter()
                        .any(|&(x0, y0, x1, y1)| x >= x0 && x < x1 && y >= y0 && y < y1)
                })
                .map_or([0.0; 4], |(c, a)| unit(c, a as f32));
            let text = if p[3] > 0.0 {
                [p[0] / p[3], p[1] / p[3], p[2] / p[3], p[3]]
            } else {
                [0.0; 4]
            };
            let behind = match look.shadow {
                Some(sh) => over(bg, unit(sh.ink, shade[i])),
                None => bg,
            };
            over(over(behind, unit(edge, o)), text)
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
        rgba: (y0..y1)
            .flat_map(|y| rgba[y * w + x0..y * w + x1].iter().copied())
            .collect(),
    });
    Some(Laid {
        sprite,
        lines,
        popped,
    })
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
    width: u32,
    height: u32,
    style: &SubtitleStyle,
) -> (Option<Sprite>, Option<Sprite>) {
    let (w, h) = (f64::from(width), f64::from(height));
    let size = (height as f32 * style.text_fraction() as f32).max(1.0);
    let bold = style.bold == Some(true);
    let look = Look {
        font: style.font.as_deref(),
        size,
        wrap: (style.wrap_width(w, h) as f32).max(size),
        align: text_align(style),
        ink: style.ink(),
        stroke: style.stroke(),
        backing: style.backing(),
        shadow: style.shadow.as_ref().map(|s| shade(s, w, h)),
        boxed: false,
        markup: Markup::Pieces(pieces, bold),
    };
    let line = Line {
        pieces: pieces.to_vec(),
        place,
        size: (width, height),
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
            backing: None,
            markup: Markup::Pieces(&word, bold),
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

pub fn paint(cues: &[Painted<'_>], width: u32, height: u32) -> Drawing {
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
                false => block(&pieces, pop, place, width, height, style),
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
    paint(&painted, width, height)
}

pub fn title(t: &Title, width: u32, height: u32) -> Vec<Sprite> {
    if t.text.trim().is_empty() {
        return Vec::new();
    }
    let (w, h) = (f64::from(width), f64::from(height));
    let zoom = t.scale();
    let size = (h * t.text_fraction() * zoom).max(1.0) as f32;
    let look = Look {
        font: t.style.font.as_deref(),
        size,
        wrap: ((t.wrap_width(w, h) * zoom) as f32).max(size),
        align: match t.align() {
            TitleAlign::Left => Align::Left,
            TitleAlign::Middle => Align::Center,
            TitleAlign::Right => Align::Right,
        },
        ink: t.ink(),
        stroke: t.stroke(),
        backing: t.backing(),
        shadow: t.style.shadow.as_ref().map(|s| shade(s, w, h)),
        boxed: true,
        markup: Markup::Title(&t.text, (h * zoom) as f32),
    };
    let Some(Laid {
        sprite: Some(sprite),
        lines,
        ..
    }) = lay_out(&look)
    else {
        return Vec::new();
    };
    let [x, y] = t.place();
    let [across, down] = t.anchor();
    let bottom = match t.kind {
        TitleKind::Text => y,
        TitleKind::LowerThird => y + t.lower_line() * (zoom - 1.0),
        TitleKind::Scroll => 1.0,
    };
    let left = (x * w) as f32 - across as f32 * look.wrap;
    let top = (bottom * h) as f32 - down as f32 * lines;
    let sprite = placed(sprite, left, top);
    let [px, py] = t.pivot();
    vec![turned(sprite, ((px * w) as f32, (py * h) as f32), t.turn())]
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
        rgba,
    }
}

pub fn backing_box(t: &Title, width: u32, height: u32) -> Option<[f64; 4]> {
    t.backing()?;
    let upright = Title {
        rotation: None,
        style: TitleStyle {
            shadow: None,
            ..t.style.clone()
        },
        ..t.clone()
    };
    let s = title(&upright, width, height).into_iter().next()?;
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
        self.rgba
            .iter()
            .flat_map(|p| {
                let a = p[3].clamp(0.0, 1.0);
                let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                [byte(p[0] * a), byte(p[1] * a), byte(p[2] * a), byte(a)]
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Light {
    Sdr,
    Pq,
    Hlg,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Coding {
    light: Light,
    wide: bool,
    matrix: Option<(f32, f32)>,
    full: bool,
}

impl Coding {
    fn of(f: &frame::Video, rgb: bool) -> Self {
        use ffi::AVColorPrimaries as P;
        use ffi::AVColorSpace as S;
        use ffi::AVColorTransferCharacteristic as T;
        // SAFETY: a live frame's plain fields.
        let (space, range, primaries, transfer) = unsafe {
            let f = &*f.as_ptr();
            (f.colorspace, f.color_range, f.color_primaries, f.color_trc)
        };
        let light = match transfer {
            T::AVCOL_TRC_SMPTE2084 => Light::Pq,
            T::AVCOL_TRC_ARIB_STD_B67 => Light::Hlg,
            _ => Light::Sdr,
        };
        let matrix = match space {
            _ if rgb => None,
            S::AVCOL_SPC_BT2020_NCL | S::AVCOL_SPC_BT2020_CL => Some((0.2627, 0.0593)),
            S::AVCOL_SPC_SMPTE170M | S::AVCOL_SPC_BT470BG => Some((0.299, 0.114)),
            S::AVCOL_SPC_UNSPECIFIED if light != Light::Sdr => Some((0.2627, 0.0593)),
            _ => Some((0.2126, 0.0722)),
        };
        Self {
            light,
            wide: primaries == P::AVCOL_PRI_BT2020 || light != Light::Sdr,
            matrix,
            full: rgb || range == ffi::AVColorRange::AVCOL_RANGE_JPEG,
        }
    }

    fn encoded(&self, srgb: [f32; 3]) -> [f32; 3] {
        if !self.wide && self.light == Light::Sdr {
            return srgb;
        }
        let lin = srgb.map(|c| {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        });
        let wide = [
            0.6274 * lin[0] + 0.3293 * lin[1] + 0.0433 * lin[2],
            0.0691 * lin[0] + 0.9195 * lin[1] + 0.0114 * lin[2],
            0.0164 * lin[0] + 0.0880 * lin[1] + 0.8956 * lin[2],
        ];
        wide.map(|l| match self.light {
            Light::Sdr => {
                if l <= 0.0031308 {
                    l * 12.92
                } else {
                    1.055 * l.powf(1.0 / 2.4) - 0.055
                }
            }
            Light::Pq => {
                let y = (l * 203.0 / 10000.0).max(0.0).powf(0.159_301_76);
                ((0.835_937_5 + 18.851_562 * y) / (1.0 + 18.6875 * y)).powf(78.84375)
            }
            Light::Hlg => {
                let e = l * 0.2647;
                if e <= 1.0 / 12.0 {
                    (3.0 * e).sqrt()
                } else {
                    0.178_832_77 * (12.0 * e - 0.284_668_92).ln() + 0.559_910_7
                }
            }
        })
    }

    fn codes(&self, srgb: [f32; 3], depth: u32) -> [f32; 3] {
        let [r, g, b] = self.encoded(srgb);
        let top = ((1u32 << depth) - 1) as f32;
        let scale = (1u32 << depth.saturating_sub(8)) as f32;
        match self.matrix {
            None => [r, g, b].map(|c| c * top),
            Some((kr, kb)) => {
                let y = kr * r + (1.0 - kr - kb) * g + kb * b;
                let cb = (b - y) / (2.0 * (1.0 - kb));
                let cr = (r - y) / (2.0 * (1.0 - kr));
                if self.full {
                    let mid = (1u32 << (depth - 1)) as f32;
                    [y * top, mid + cb * top, mid + cr * top]
                } else {
                    [
                        (16.0 + 219.0 * y) * scale,
                        (128.0 + 224.0 * cb) * scale,
                        (128.0 + 224.0 * cr) * scale,
                    ]
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Comp {
    plane: usize,
    step: usize,
    offset: usize,
    shift: u32,
    depth: u32,
    log2w: u32,
    log2h: u32,
    role: usize,
}

struct Layout {
    comps: Vec<Comp>,
    rgb: bool,
}

fn layout(pixel: format::Pixel) -> Result<Layout> {
    let unknown = || Error::Invalid(format!("subtitles can't be drawn into {pixel:?} pictures"));
    let desc = pixel.descriptor().ok_or_else(unknown)?;
    // SAFETY: FFmpeg's static descriptor of the format.
    let d = unsafe { &*desc.as_ptr() };
    let bad = (ffi::AV_PIX_FMT_FLAG_HWACCEL
        | ffi::AV_PIX_FMT_FLAG_BE
        | ffi::AV_PIX_FMT_FLAG_BITSTREAM
        | ffi::AV_PIX_FMT_FLAG_PAL
        | ffi::AV_PIX_FMT_FLAG_FLOAT) as u64;
    if d.flags & bad != 0 || d.nb_components < 3 {
        return Err(unknown());
    }
    let rgb = d.flags & ffi::AV_PIX_FMT_FLAG_RGB as u64 != 0;
    let comps = (0..usize::from(d.nb_components))
        .map(|i| {
            let c = d.comp[i];
            let chroma = !rgb && (i == 1 || i == 2);
            Comp {
                plane: c.plane as usize,
                step: c.step as usize,
                offset: c.offset as usize,
                shift: c.shift as u32,
                depth: c.depth as u32,
                log2w: if chroma {
                    u32::from(d.log2_chroma_w)
                } else {
                    0
                },
                log2h: if chroma {
                    u32::from(d.log2_chroma_h)
                } else {
                    0
                },
                role: i,
            }
        })
        .collect::<Vec<_>>();
    if comps.iter().any(|c| c.depth + c.shift > 32) {
        return Err(unknown());
    }
    Ok(Layout { comps, rgb })
}

fn visit(
    sprites: &[Sprite],
    ((dx, dy), alpha): Placing,
    (w, h): (i32, i32),
    c: &Comp,
    coding: &Coding,
    mut each: impl FnMut(usize, usize, f32, f32),
) {
    let (sw, sh) = (1i32 << c.log2w, 1i32 << c.log2h);
    for s in sprites {
        let (sx0, sy0) = (s.x + dx, s.y + dy);
        let x0 = sx0.max(0) / sw;
        let y0 = sy0.max(0) / sh;
        let x1 = ((sx0 + s.width as i32).min(w) + sw - 1) / sw;
        let y1 = ((sy0 + s.height as i32).min(h) + sh - 1) / sh;
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        for cy in y0..y1 {
            for cx in x0..x1 {
                let (mut a, mut rgb, mut n) = (0.0f32, [0.0f32; 3], 0.0f32);
                for y in cy * sh..(cy * sh + sh).min(h) {
                    for x in cx * sw..(cx * sw + sw).min(w) {
                        let p = s.at(x - dx, y - dy);
                        a += p[3];
                        for i in 0..3 {
                            rgb[i] += p[i] * p[3];
                        }
                        n += 1.0;
                    }
                }
                if a <= 0.0 || n <= 0.0 {
                    continue;
                }
                let color = rgb.map(|v| v / a);
                let target = if c.role == 3 {
                    ((1u32 << c.depth) - 1) as f32
                } else {
                    coding.codes(color, c.depth)[c.role]
                };
                each(cx as usize, cy as usize, target, a / n * alpha);
            }
        }
    }
}

unsafe fn sample(row: *mut u8, c: &Comp, x: usize) -> (*mut u8, usize) {
    let bytes = (c.shift + c.depth).div_ceil(8) as usize;
    // SAFETY: the caller keeps `x` inside the row.
    (unsafe { row.add(x * c.step + c.offset) }, bytes)
}

unsafe fn read(at: *const u8, bytes: usize) -> u32 {
    let mut v = [0u8; 4];
    // SAFETY: the caller passes `bytes` readable bytes.
    unsafe { std::ptr::copy_nonoverlapping(at, v.as_mut_ptr(), bytes) };
    u32::from_le_bytes(v)
}

unsafe fn write(at: *mut u8, bytes: usize, v: u32) {
    // SAFETY: the caller passes `bytes` writable bytes.
    unsafe { std::ptr::copy_nonoverlapping(v.to_le_bytes().as_ptr(), at, bytes) };
}

fn mask(c: &Comp) -> u32 {
    if c.depth >= 32 {
        u32::MAX
    } else {
        ((1u32 << c.depth) - 1) << c.shift
    }
}

pub(crate) fn blend_at(f: &mut frame::Video, sprites: &[Sprite], shift: Placing) -> Result<()> {
    let layout = layout(f.format())?;
    // SAFETY: a live frame.
    let ret = unsafe { ffi::av_frame_make_writable(f.as_mut_ptr()) };
    if ret < 0 {
        return Err(Error::Ffmpeg {
            context: "making a picture writable".into(),
            source: crate::ffmpeg::Error::from(ret),
        });
    }
    let coding = Coding::of(f, layout.rgb);
    let size = (f.width() as i32, f.height() as i32);
    for c in &layout.comps {
        let mask = mask(c);
        // SAFETY: the plane's pointer and stride of a writable frame in memory.
        let (data, stride) = unsafe {
            let raw = f.as_mut_ptr();
            ((*raw).data[c.plane], (*raw).linesize[c.plane] as isize)
        };
        visit(sprites, shift, size, c, &coding, |x, y, target, a| {
            // SAFETY: `visit` keeps `x` and `y` inside the component's plane.
            unsafe {
                let row = data.offset(y as isize * stride);
                let (at, bytes) = sample(row, c, x);
                let old = read(at, bytes);
                let v = ((old & mask) >> c.shift) as f32;
                let new = (v + (target - v) * a).round().max(0.0) as u32;
                let new = (new << c.shift) & mask;
                write(at, bytes, (old & !mask) | new);
            }
        });
    }
    Ok(())
}

pub(crate) fn bounds(
    sprites: &[Sprite],
    ((dx, dy), _): Placing,
    (w, h): (u32, u32),
) -> Option<(i32, i32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for s in sprites {
        x0 = x0.min(s.x + dx);
        y0 = y0.min(s.y + dy);
        x1 = x1.max(s.x + dx + s.width as i32);
        y1 = y1.max(s.y + dy + s.height as i32);
    }
    let (x0, y0) = (x0.max(0) & !1, y0.max(0) & !1);
    let x1 = ((x1.saturating_add(1)) & !1).min(w as i32);
    let y1 = ((y1.saturating_add(1)) & !1).min(h as i32);
    (x1 > x0 && y1 > y0).then(|| (x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
}

pub(crate) fn overlay(
    pixel: format::Pixel,
    like: &frame::Video,
    sprites: &[Sprite],
    ((dx, dy), level): Placing,
    (ax, ay, w, h): (i32, i32, u32, u32),
) -> Result<(frame::Video, frame::Video)> {
    let layout = layout(pixel)?;
    let shift = ((dx - ax, dy - ay), level);
    let mut color = frame::Video::new(pixel, w, h);
    let mut alpha = frame::Video::new(pixel, w, h);
    for f in [&mut color, &mut alpha] {
        for p in 0..f.planes() {
            f.data_mut(p).fill(0);
        }
    }
    let coding = Coding::of(like, layout.rgb);
    for c in &layout.comps {
        let top = ((1u32 << c.depth) - 1) as f32;
        let mask = mask(c);
        let [(colors, color_stride), (covers, cover_stride)] = [&mut color, &mut alpha].map(|f| {
            // SAFETY: the plane's pointer and stride of a frame in memory.
            unsafe {
                let raw = f.as_mut_ptr();
                ((*raw).data[c.plane], (*raw).linesize[c.plane] as isize)
            }
        });
        let put =
            |old: u32, v: f32| (old & !mask) | (((v.round().max(0.0) as u32) << c.shift) & mask);
        visit(
            sprites,
            shift,
            (w as i32, h as i32),
            c,
            &coding,
            |x, y, target, a| {
                // SAFETY: `visit` keeps `x` and `y` inside the component's plane.
                unsafe {
                    let (color_at, bytes) = sample(colors.offset(y as isize * color_stride), c, x);
                    let (cover_at, _) = sample(covers.offset(y as isize * cover_stride), c, x);
                    let (color_old, cover_old) = (read(color_at, bytes), read(cover_at, bytes));
                    let under = ((color_old & mask) >> c.shift) as f32;
                    let below = ((cover_old & mask) >> c.shift) as f32 / top;
                    let cover = a + below * (1.0 - a);
                    let value = if cover > 0.0 {
                        (target * a + under * below * (1.0 - a)) / cover
                    } else {
                        0.0
                    };
                    write(color_at, bytes, put(color_old, value));
                    write(cover_at, bytes, put(cover_old, cover * top));
                }
            },
        );
    }
    Ok((color, alpha))
}

#[cfg(test)]
mod tests {
    use super::{Drawing, draw};
    use vidcut_core::model::{Place, SubtitleStyle};
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
            &SubtitleStyle::default(),
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
