use std::sync::{Mutex, OnceLock};

use cosmic_text::{
    Align, Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Style, SwashCache,
    UnderlineStyle, Weight, Wrap,
};
use vidcut_core::model::Place;
use vidcut_core::subtitles::{Tag, Token, tokens};

use crate::error::{Error, Result};
use crate::ffmpeg::{ffi, format, frame};

const SIZE: f32 = 0.055;
const LINE: f32 = 1.31;
const OUTLINE: f32 = 0.05;
const WIDTH: f32 = 0.9;

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

fn fonts() -> &'static Mutex<Fonts> {
    static FONTS: OnceLock<Mutex<Fonts>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut system = FontSystem::new();
        let installed = FAMILIES.iter().find(|name| {
            system
                .db()
                .faces()
                .any(|f| f.families.iter().any(|(n, _)| n == *name))
        });
        if let Some(name) = installed {
            system.db_mut().set_sans_serif_family(*name);
        }
        Mutex::new(Fonts {
            system,
            cache: SwashCache::new(),
        })
    })
}

fn hex_color(hex: &str) -> Option<Color> {
    let hex = hex.trim_start_matches('#');
    let part = |at: usize| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok();
    Some(Color::rgb(part(0)?, part(2)?, part(4)?))
}

fn spans(text: &str) -> Vec<(String, Attrs<'static>)> {
    let base = Attrs::new()
        .family(Family::SansSerif)
        .color(Color::rgb(255, 255, 255));
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
            Token::Open(Tag::Color(hex)) => {
                colors.push(hex_color(&hex).unwrap_or(Color::rgb(255, 255, 255)))
            }
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

fn block(text: &str, place: Place, width: u32, height: u32, f: &mut Fonts) -> Option<Sprite> {
    let size = (height as f32 * SIZE).max(1.0);
    let wrap = (width as f32 * WIDTH).max(size);
    let mut buffer = Buffer::new(&mut f.system, Metrics::new(size, size * LINE));
    buffer.set_wrap(Wrap::WordOrGlyph);
    buffer.set_size(Some(wrap), None);
    let spans = spans(text);
    let base = Attrs::new().family(Family::SansSerif);
    buffer.set_rich_text(
        spans.iter().map(|(s, a)| (s.as_str(), a.clone())),
        &base,
        Shaping::Advanced,
        Some(Align::Center),
    );
    buffer.shape_until_scroll(&mut f.system, false);
    let lines: f32 = buffer.layout_runs().map(|r| r.line_height).sum();
    if lines <= 0.0 {
        return None;
    }
    let outline = size * OUTLINE;
    let pad = outline.ceil() as i32 + 2;
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
    let ring = dilate(&alpha, w, h, outline);
    let rgba: Vec<[f32; 4]> = canvas
        .iter()
        .zip(&ring)
        .map(|(p, &o)| {
            let a = p[3] + o * (1.0 - p[3]);
            if a <= 0.0 {
                [0.0; 4]
            } else {
                [p[0] / a, p[1] / a, p[2] / a, a]
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
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let centre = match place {
        Place::Top => 0.2,
        Place::Middle => 0.5,
        Place::Bottom => 0.8,
    } * height as f32;
    let left = ((width as f32 - wrap) / 2.0).round() as i32 - pad;
    let top = (centre - lines / 2.0).round() as i32 - pad;
    let crop = (y0..y1)
        .flat_map(|y| rgba[y * w + x0..y * w + x1].iter().copied())
        .collect();
    Some(Sprite {
        x: left + x0 as i32,
        y: top + y0 as i32,
        width: (x1 - x0) as u32,
        height: (y1 - y0) as u32,
        rgba: crop,
    })
}

pub fn draw(cues: &[(&str, Place)], width: u32, height: u32) -> Vec<Sprite> {
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
            block(&text.join("\n"), place, width, height, &mut f)
        })
        .collect()
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
        let mask = if c.depth >= 32 {
            u32::MAX
        } else {
            ((1u32 << c.depth) - 1) << c.shift
        };
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
        let planes = [&mut color, &mut alpha].map(|f| {
            // SAFETY: the plane's pointer and stride of a frame in memory.
            unsafe {
                let raw = f.as_mut_ptr();
                ((*raw).data[c.plane], (*raw).linesize[c.plane] as isize)
            }
        });
        visit(
            sprites,
            (w as i32, h as i32),
            c,
            &coding,
            |x, y, target, a| {
                for ((data, stride), v) in planes.iter().zip([target, a * top]) {
                    // SAFETY: `visit` keeps `x` and `y` inside the component's plane.
                    unsafe {
                        let row = data.offset(y as isize * stride);
                        let (at, bytes) = sample(row, c, x);
                        let old = read(at, bytes);
                        write(at, bytes, old | ((v.round().max(0.0) as u32) << c.shift));
                    }
                }
            },
        );
    }
    Ok((color, alpha))
}
