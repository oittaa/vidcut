use std::sync::{Mutex, OnceLock};

use cosmic_text::{
    Align, Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Style, SwashCache,
    UnderlineStyle, Weight, Wrap,
};
use vidcut_core::model::{
    Place, SubtitleStyle, Title, TitleAlign, TitleKind, TitleShadow, TitleStyle,
};
use vidcut_core::subtitles::{Tag, Token, rgb, tokens};

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

fn spans<'a>(text: &str, family: Family<'a>, ink: Color) -> Vec<(String, Attrs<'a>)> {
    let base = Attrs::new().family(family).color(ink);
    let (mut italic, mut bold, mut under) = (0u32, 0u32, 0u32);
    let mut colors: Vec<Color> = Vec::new();
    let mut out = Vec::new();
    for t in tokens(text) {
        match t {
            Token::Text(s) => {
                let mut a = base.clone();
                if italic > 0 {
                    a = a.style(Style::Italic);
                }
                if bold > 0 {
                    a = a.weight(Weight::BOLD);
                }
                if under > 0 {
                    a = a.underline(UnderlineStyle::Single);
                }
                if let Some(c) = colors.last() {
                    a = a.color(*c);
                }
                out.push((s, a));
            }
            Token::Open(Tag::I) => italic += 1,
            Token::Open(Tag::B) => bold += 1,
            Token::Open(Tag::U) => under += 1,
            Token::Open(Tag::Color(hex)) => colors.push(rgb(&hex).map_or(ink, color)),
            Token::Close(Tag::I) => italic = italic.saturating_sub(1),
            Token::Close(Tag::B) => bold = bold.saturating_sub(1),
            Token::Close(Tag::U) => under = under.saturating_sub(1),
            Token::Close(Tag::Color(_)) => {
                colors.pop();
            }
        }
    }
    out
}

fn dilate(alpha: &[f32], w: usize, h: usize, radius: f32) -> Vec<f32> {
    let reach = radius.ceil() as i32 + 1;
    let mut out = alpha.to_vec();
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            let weight = (radius + 0.5 - ((dx * dx + dy * dy) as f32).sqrt()).clamp(0.0, 1.0);
            if weight <= 0.0 || (dx == 0 && dy == 0) {
                continue;
            }
            for y in 0..h as i32 {
                let sy = y + dy;
                if sy < 0 || sy >= h as i32 {
                    continue;
                }
                let (row, src) = (y as usize * w, sy as usize * w);
                let (x0, x1) = (
                    (-dx).max(0) as usize,
                    (w as i32 - dx.max(0)).max(0) as usize,
                );
                for x in x0..x1 {
                    let v = alpha[src + (x as i32 + dx) as usize] * weight;
                    if v > out[row + x] {
                        out[row + x] = v;
                    }
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
    rich: bool,
}

#[derive(Debug, Clone, Copy)]
struct Shade {
    ink: [u8; 3],
    opacity: f32,
    shift: (i32, i32),
    sigma: f32,
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

fn lay_out(text: &str, look: &Look, f: &mut Fonts) -> Option<(Sprite, f32)> {
    let size = look.size;
    let wrap = look.wrap;
    let mut buffer = Buffer::new(
        &mut f.system,
        Metrics::new(size, size * SubtitleStyle::LINE as f32),
    );
    buffer.set_wrap(Wrap::WordOrGlyph);
    buffer.set_size(Some(wrap), None);
    let installed = look.font.and_then(|name| {
        f.system.db().faces().find_map(|face| {
            face.families
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(n, _)| n.clone())
        })
    });
    let family = match installed.as_deref() {
        Some(name) => Family::Name(name),
        None => Family::SansSerif,
    };
    let spans = if look.rich {
        spans(text, family, color(look.ink))
    } else {
        vec![(
            text.to_string(),
            Attrs::new().family(family).color(color(look.ink)),
        )]
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
    let (cw, ch) = (wrap.ceil() as i32 + 2 * pad, lines.ceil() as i32 + 2 * pad);
    let (w, h) = (cw as usize, ch as usize);
    let mut canvas = vec![[0.0f32; 4]; w * h];
    buffer.draw(
        &mut f.system,
        &mut f.cache,
        Color::rgb(255, 255, 255),
        |x, y, bw, bh, color| {
            let a = f32::from(color.a()) / 255.0;
            if a <= 0.0 {
                return;
            }
            let rgb = [color.r(), color.g(), color.b()].map(|c| f32::from(c) / 255.0);
            for py in y..y + bh as i32 {
                for px in x..x + bw as i32 {
                    let (cx, cy) = (px + pad, py + pad);
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
    );
    let alpha: Vec<f32> = canvas.iter().map(|p| p[3]).collect();
    let (ring, edge) = match stroke {
        Some((rgb, _)) => (dilate(&alpha, w, h, outline), rgb),
        None => (vec![0.0; alpha.len()], [0, 0, 0]),
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
            let (x, y) = ((i % w) as i32 - pad, (i / w) as i32 - pad);
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
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let crop = (y0..y1)
        .flat_map(|y| rgba[y * w + x0..y * w + x1].iter().copied())
        .collect();
    Some((
        Sprite {
            x: x0 as i32 - pad,
            y: y0 as i32 - pad,
            width: (x1 - x0) as u32,
            height: (y1 - y0) as u32,
            rgba: crop,
        },
        lines,
    ))
}

fn placed(mut sprite: Sprite, left: f32, top: f32) -> Sprite {
    sprite.x += left.round() as i32;
    sprite.y += top.round() as i32;
    sprite
}

fn block(
    text: &str,
    place: Place,
    width: u32,
    height: u32,
    style: &SubtitleStyle,
    f: &mut Fonts,
) -> Option<Sprite> {
    let size = (height as f32 * style.text_fraction() as f32).max(1.0);
    let look = Look {
        font: style.font.as_deref(),
        size,
        wrap: (style.wrap_width(f64::from(width), f64::from(height)) as f32).max(size),
        align: Align::Center,
        ink: style.ink(),
        stroke: style.stroke(),
        backing: style.backing(),
        shadow: None,
        boxed: false,
        rich: true,
    };
    let (sprite, lines) = lay_out(text, &look, f)?;
    let text_top = style.block_top(place, f64::from(height), f64::from(lines)) as f32;
    Some(placed(sprite, (width as f32 - look.wrap) / 2.0, text_top))
}

pub fn draw(cues: &[(&str, Place)], width: u32, height: u32, style: &SubtitleStyle) -> Vec<Sprite> {
    let mut f = fonts().lock().unwrap_or_else(|e| e.into_inner());
    [Place::Top, Place::Middle, Place::Bottom]
        .into_iter()
        .filter_map(|place| {
            let text: Vec<&str> = cues
                .iter()
                .filter(|(_, p)| *p == place)
                .map(|(t, _)| *t)
                .collect();
            if text.is_empty() {
                return None;
            }
            block(&text.join("\n"), place, width, height, style, &mut f)
        })
        .collect()
}

pub fn title(t: &Title, width: u32, height: u32) -> Vec<Sprite> {
    if t.text.trim().is_empty() {
        return Vec::new();
    }
    let mut f = fonts().lock().unwrap_or_else(|e| e.into_inner());
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
        shadow: t.style.shadow.as_ref().map(|s| {
            let [dx, dy] = s.shift();
            Shade {
                ink: s.ink(),
                opacity: s.alpha() as f32,
                shift: ((dx * w).round() as i32, (dy * h).round() as i32),
                sigma: (s.softness() * h / TitleShadow::BLUR_PER_PICTURE) as f32,
            }
        }),
        boxed: true,
        rich: false,
    };
    let Some((sprite, lines)) = lay_out(&t.text, &look, &mut f) else {
        return Vec::new();
    };
    let [x, y] = t.place();
    let [across, down] = t.anchor();
    let bottom = match t.kind {
        TitleKind::Text => y,
        TitleKind::LowerThird => y + t.text_fraction() * SubtitleStyle::LINE * (zoom - 1.0),
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
    (w, h): (i32, i32),
    c: &Comp,
    coding: &Coding,
    mut each: impl FnMut(usize, usize, f32, f32),
) {
    let (sw, sh) = (1i32 << c.log2w, 1i32 << c.log2h);
    for s in sprites {
        let x0 = s.x.max(0) / sw;
        let y0 = s.y.max(0) / sh;
        let x1 = ((s.x + s.width as i32).min(w) + sw - 1) / sw;
        let y1 = ((s.y + s.height as i32).min(h) + sh - 1) / sh;
        for cy in y0..y1 {
            for cx in x0..x1 {
                let (mut a, mut rgb, mut n) = (0.0f32, [0.0f32; 3], 0.0f32);
                for y in cy * sh..(cy * sh + sh).min(h) {
                    for x in cx * sw..(cx * sw + sw).min(w) {
                        let p = s.at(x, y);
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
                each(cx as usize, cy as usize, target, a / n);
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

pub(crate) fn blend(f: &mut frame::Video, sprites: &[Sprite]) -> Result<()> {
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
        visit(sprites, size, c, &coding, |x, y, target, a| {
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

pub(crate) fn overlay(
    pixel: format::Pixel,
    like: &frame::Video,
    sprites: &[Sprite],
) -> Result<(frame::Video, frame::Video)> {
    let layout = layout(pixel)?;
    let (w, h) = (like.width(), like.height());
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
