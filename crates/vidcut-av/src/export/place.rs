use vidcut_core::model::Title;
use vidcut_core::place::{Placing, ahead, apply, chain, lettered};
use vidcut_core::render::{Placed, WHOLE};

use crate::burn::Sprite;
use crate::error::{Error, Result};
use crate::ffmpeg::{ffi, format, frame};

#[derive(Debug, Clone, Copy)]
struct Comp {
    plane: usize,
    step: usize,
    offset: usize,
    shift: u32,
    depth: u32,
    log2w: u32,
    log2h: u32,
}

impl Comp {
    fn wide(&self) -> bool {
        self.depth + self.shift > 8
    }

    fn read(&self, data: &[u8], at: usize) -> f32 {
        let raw = if self.wide() {
            u32::from(u16::from_le_bytes([data[at], data[at + 1]]))
        } else {
            u32::from(data[at])
        };
        ((raw >> self.shift) & ((1 << self.depth) - 1)) as f32
    }

    fn write(&self, data: &mut [u8], at: usize, v: f32) {
        let top = ((1u32 << self.depth) - 1) as f32;
        let v = (v.round().clamp(0.0, top) as u32) << self.shift;
        if self.wide() {
            data[at..at + 2].copy_from_slice(&(v as u16).to_le_bytes());
        } else {
            data[at] = v as u8;
        }
    }
}

fn comps(pixel: format::Pixel) -> Result<Vec<Comp>> {
    let unknown = || Error::Invalid(format!("pictures in {pixel:?} can't be placed"));
    let desc = pixel.descriptor().ok_or_else(unknown)?;
    // SAFETY: FFmpeg's static descriptor of the format.
    let d = unsafe { &*desc.as_ptr() };
    let bad = (ffi::AV_PIX_FMT_FLAG_HWACCEL
        | ffi::AV_PIX_FMT_FLAG_BE
        | ffi::AV_PIX_FMT_FLAG_BITSTREAM
        | ffi::AV_PIX_FMT_FLAG_PAL
        | ffi::AV_PIX_FMT_FLAG_FLOAT) as u64;
    if d.flags & bad != 0 {
        return Err(unknown());
    }
    let rgb = d.flags & ffi::AV_PIX_FMT_FLAG_RGB as u64 != 0;
    let comps: Vec<Comp> = (0..usize::from(d.nb_components))
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
            }
        })
        .collect();
    let shares = |a: &Comp, b: &Comp| {
        let bytes = |c: &Comp| if c.wide() { 2 } else { 1 };
        a.plane == b.plane && a.offset < b.offset + bytes(b) && b.offset < a.offset + bytes(a)
    };
    let overlap = comps
        .iter()
        .enumerate()
        .any(|(i, a)| comps[i + 1..].iter().any(|b| shares(a, b)));
    if comps.is_empty() || overlap || comps.iter().any(|c| c.depth + c.shift > 16) {
        return Err(unknown());
    }
    Ok(comps)
}

fn copy(under: &frame::Video) -> frame::Video {
    let mut out = frame::Video::new(under.format(), under.width(), under.height());
    for p in 0..under.planes() {
        let rows = under.plane_height(p) as usize;
        let (from, to) = (under.stride(p), out.stride(p));
        let bytes = from.min(to);
        let src = under.data(p);
        let dst = out.data_mut(p);
        for y in 0..rows {
            dst[y * to..y * to + bytes].copy_from_slice(&src[y * from..y * from + bytes]);
        }
    }
    // SAFETY: `out` and `under` are valid frames.
    unsafe { ffi::av_frame_copy_props(out.as_mut_ptr(), under.as_ptr()) };
    out
}

pub(crate) struct Layer<'a> {
    pub(crate) picture: &'a frame::Video,
    pub(crate) placing: &'a Placing,
    pub(crate) fitted: [f64; 4],
    pub(crate) weight: u32,
}

fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get().clamp(1, 8))
}

fn bands(rows: std::ops::Range<usize>) -> Vec<std::ops::Range<usize>> {
    let band = rows.len().div_ceil(threads()).max(1);
    rows.clone()
        .step_by(band)
        .map(|a| a..(a + band).min(rows.end))
        .collect()
}

pub(crate) fn over(under: &frame::Video, layer: &Layer) -> Result<frame::Video> {
    let pixel = under.format();
    if layer.picture.format() != pixel {
        return Err(Error::Invalid(
            "pictures of different formats can't be placed over each other".into(),
        ));
    }
    let comps = comps(pixel)?;
    let mut out = copy(under);
    let size = (under.width(), under.height());
    let Some(area) = layer.placing.bounds((f64::from(size.0), f64::from(size.1))) else {
        return Ok(out);
    };
    for p in 0..out.planes() {
        let mine: Vec<Shading> = comps
            .iter()
            .filter(|c| c.plane == p)
            .map(|c| Shading::of(*c, layer, size, area))
            .collect();
        let Some(rows) = mine.first().map(|s| s.rows(out.plane_height(p) as usize)) else {
            continue;
        };
        let stride = out.stride(p);
        let mut rest = &mut out.data_mut(p)[rows.start * stride..];
        std::thread::scope(|scope| {
            for band in bands(rows) {
                let (chunk, next) = rest.split_at_mut((band.len() * stride).min(rest.len()));
                rest = next;
                let mine = &mine;
                scope.spawn(move || {
                    for (k, y) in band.enumerate() {
                        let row = &mut chunk[k * stride..][..stride];
                        for s in mine {
                            for x in s.columns() {
                                if let Some((shown, a)) = s.pixel(x, y) {
                                    let to = x * s.comp.step + s.comp.offset;
                                    let under = s.comp.read(row, to);
                                    s.comp.write(row, to, under + (shown - under) * a);
                                }
                            }
                        }
                    }
                });
            }
        });
    }
    Ok(out)
}

fn texel(s: &Sprite, i: i64, j: i64) -> [f64; 4] {
    if i < 0 || j < 0 || i >= i64::from(s.width) || j >= i64::from(s.height) {
        return [0.0; 4];
    }
    let p = s.rgba[j as usize * s.width as usize + i as usize].map(f64::from);
    [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]]
}

fn bilinear(s: &Sprite, [x, y]: [f64; 2]) -> [f64; 4] {
    let (fx, fy) = (x - 0.5, y - 0.5);
    let (kx, ky) = (fx.floor(), fy.floor());
    let (ax, ay) = (fx - kx, fy - ky);
    let (kx, ky) = (kx as i64, ky as i64);
    let mut out = [0.0; 4];
    for (dx, dy, k) in [
        (0, 0, (1.0 - ax) * (1.0 - ay)),
        (1, 0, ax * (1.0 - ay)),
        (0, 1, (1.0 - ax) * ay),
        (1, 1, ax * ay),
    ] {
        for (o, v) in out.iter_mut().zip(texel(s, kx + dx, ky + dy)) {
            *o += v * k;
        }
    }
    out
}

pub(crate) fn title(
    s: &Sprite,
    t: &Title,
    placed: &[Placed],
    dy: i32,
    (width, height): (u32, u32),
) -> Option<Sprite> {
    let frame = (f64::from(width), f64::from(height));
    let placing = Placing::whole(placed, frame);
    let [px, py] = t.pivot();
    let own = lettered(
        [px * frame.0, py * frame.1],
        1.0,
        t.turn().to_radians(),
        [f64::from(s.x), f64::from(s.y + dy)],
    );
    let map = chain(&placing.inner(frame), &own);
    let (sw, sh) = (f64::from(s.width), f64::from(s.height));
    let corners = [
        [-0.5, -0.5],
        [sw + 0.5, -0.5],
        [sw + 0.5, sh + 0.5],
        [-0.5, sh + 0.5],
    ]
    .map(|c| ahead(&map, c));
    let [bx0, by0, bx1, by1] = placing.bounds(frame)?.map(f64::from);
    let lo = |i: usize| corners.iter().map(|c| c[i]).fold(f64::INFINITY, f64::min) - 1.0;
    let hi = |i: usize| {
        corners
            .iter()
            .map(|c| c[i])
            .fold(f64::NEG_INFINITY, f64::max)
            + 1.0
    };
    let x0 = lo(0).floor().clamp(bx0, bx1) as u32;
    let x1 = hi(0).ceil().clamp(bx0, bx1) as u32;
    let y0 = lo(1).floor().clamp(by0, by1) as u32;
    let y1 = hi(1).ceil().clamp(by0, by1) as u32;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let [a, b, _, c, d, _] = map;
    let taps = |u: f64, v: f64| (u.hypot(v).ceil() as usize).clamp(1, 8);
    let (tx, ty) = (taps(a, c), taps(b, d));
    let shade = |x: u32, y: u32| -> [f32; 4] {
        let (cx, cy) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
        let Some((_, cover)) = placing.at(cx, cy) else {
            return [0.0; 4];
        };
        let [qx, qy] = apply(&map, [cx, cy]);
        let mut sum = [0.0; 4];
        for j in 0..ty {
            for i in 0..tx {
                let ox = (i as f64 + 0.5) / tx as f64 - 0.5;
                let oy = (j as f64 + 0.5) / ty as f64 - 0.5;
                let p = bilinear(s, [qx + a * ox + b * oy, qy + c * ox + d * oy]);
                for (o, v) in sum.iter_mut().zip(p) {
                    *o += v;
                }
            }
        }
        let alpha = sum[3] / (tx * ty) as f64 * cover;
        if alpha <= 0.0 {
            return [0.0; 4];
        }
        [
            (sum[0] / sum[3]) as f32,
            (sum[1] / sum[3]) as f32,
            (sum[2] / sum[3]) as f32,
            alpha as f32,
        ]
    };
    let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let mut rgba = vec![[0.0f32; 4]; w * h];
    std::thread::scope(|scope| {
        let mut rest = rgba.as_mut_slice();
        for band in bands(0..h) {
            let (chunk, next) = rest.split_at_mut(band.len() * w);
            rest = next;
            let shade = &shade;
            scope.spawn(move || {
                for (k, row) in band.enumerate() {
                    for (col, p) in chunk[k * w..][..w].iter_mut().enumerate() {
                        *p = shade(x0 + col as u32, y0 + row as u32);
                    }
                }
            });
        }
    });
    Some(Sprite {
        x: x0 as i32,
        y: y0 as i32,
        width: w as u32,
        height: h as u32,
        rgba,
    })
}

pub(crate) fn overlay(
    pixel: format::Pixel,
    layer: &Layer,
    size: (u32, u32),
    area: [u32; 4],
) -> Result<(frame::Video, frame::Video)> {
    let comps = comps(pixel)?;
    let (w, h) = (area[2] - area[0], area[3] - area[1]);
    let mut color = frame::Video::new(pixel, w, h);
    let mut alpha = frame::Video::new(pixel, w, h);
    for f in [&mut color, &mut alpha] {
        for p in 0..f.planes() {
            f.data_mut(p).fill(0);
        }
    }
    for p in 0..color.planes() {
        let mine: Vec<Shading> = comps
            .iter()
            .filter(|c| c.plane == p)
            .map(|c| Shading::of(*c, layer, size, area))
            .collect();
        let Some(first) = mine.first() else {
            continue;
        };
        let top = (area[1] >> first.comp.log2h) as usize;
        let rows = top..top + color.plane_height(p) as usize;
        let (cs, as_) = (color.stride(p), alpha.stride(p));
        let mut colors = color.data_mut(p);
        let mut covers = alpha.data_mut(p);
        std::thread::scope(|scope| {
            for band in bands(rows) {
                let (cc, cn) = colors.split_at_mut((band.len() * cs).min(colors.len()));
                let (ac, an) = covers.split_at_mut((band.len() * as_).min(covers.len()));
                (colors, covers) = (cn, an);
                let mine = &mine;
                scope.spawn(move || {
                    for (k, y) in band.enumerate() {
                        let crow = &mut cc[k * cs..][..cs];
                        let arow = &mut ac[k * as_..][..as_];
                        for s in mine {
                            let left = (area[0] >> s.comp.log2w) as usize;
                            let full = ((1u32 << s.comp.depth) - 1) as f32;
                            for x in s.columns() {
                                if let Some((shown, a)) = s.pixel(x, y) {
                                    let to = (x - left) * s.comp.step + s.comp.offset;
                                    s.comp.write(crow, to, shown);
                                    s.comp.write(arow, to, a * full);
                                }
                            }
                        }
                    }
                });
            }
        });
    }
    Ok((color, alpha))
}

struct Shading<'a> {
    comp: Comp,
    data: &'a [u8],
    stride: usize,
    placing: &'a Placing,
    fitted: [f64; 4],
    level: f64,
    scale: (f64, f64),
    grid: (usize, usize),
    span: (usize, usize),
    lines: (usize, usize),
    taps: (usize, usize),
    lo: (f64, f64),
    hi: (f64, f64),
}

impl<'a> Shading<'a> {
    fn of(comp: Comp, layer: &Layer<'a>, size: (u32, u32), area: [u32; 4]) -> Self {
        let p = comp.plane;
        let (across, down) = (f64::from(1u32 << comp.log2w), f64::from(1u32 << comp.log2h));
        let picture = layer.picture;
        let grid = (
            picture.width().div_ceil(1 << comp.log2w) as usize,
            picture.height().div_ceil(1 << comp.log2h) as usize,
        );
        let [fx, fy, fw, fh] = layer.fitted;
        let [dux, duy, dvx, dvy] = layer.placing.step();
        let taps = |a: f64, b: f64| (a.hypot(b).ceil() as usize).clamp(1, 8);
        let x0 = (area[0] >> comp.log2w) as usize;
        let x1 = area[2].min(size.0).div_ceil(1 << comp.log2w) as usize;
        Self {
            comp,
            data: picture.data(p),
            stride: picture.stride(p),
            placing: layer.placing,
            fitted: layer.fitted,
            level: f64::from(layer.weight) / f64::from(WHOLE),
            scale: (across, down),
            grid,
            span: (x0, x1),
            lines: (
                (area[1] >> comp.log2h) as usize,
                area[3].min(size.1).div_ceil(1 << comp.log2h) as usize,
            ),
            taps: (
                taps(dux * fw, dvx * fh * across / down),
                taps(duy * fw * down / across, dvy * fh),
            ),
            lo: (fx / across, fy / down),
            hi: (
                ((fx + fw) / across - 1.0).min(grid.0 as f64 - 1.0),
                ((fy + fh) / down - 1.0).min(grid.1 as f64 - 1.0),
            ),
        }
    }

    fn rows(&self, plane: usize) -> std::ops::Range<usize> {
        let (y0, y1) = self.lines;
        y0..y1.min(plane).max(y0)
    }

    fn columns(&self) -> std::ops::Range<usize> {
        self.span.0..self.span.1
    }

    fn pixel(&self, x: usize, y: usize) -> Option<(f32, f32)> {
        let (across, down) = self.scale;
        let (cx, cy) = ((x as f64 + 0.5) * across, (y as f64 + 0.5) * down);
        let (_, cover) = self.placing.at(cx, cy)?;
        let [fx, fy, fw, fh] = self.fitted;
        let back = self.placing.levels.last().map_or([0.0; 6], |l| l.back);
        let (tx, ty) = self.taps;
        let (cols, rows) = self.grid;
        let c = &self.comp;
        let at = |col: usize, line: usize| line * self.stride + col * c.step + c.offset;
        let read = |col, line| c.read(self.data, at(col, line));
        let mut sum = 0.0f32;
        for j in 0..ty {
            for i in 0..tx {
                let px = x as f64 * across + (i as f64 + 0.5) / tx as f64 * across;
                let py = y as f64 * down + (j as f64 + 0.5) / ty as f64 * down;
                let u = back[0] * px + back[1] * py + back[2];
                let v = back[3] * px + back[4] * py + back[5];
                let sx = ((fx + u * fw) / across - 0.5).clamp(self.lo.0, self.hi.0.max(self.lo.0));
                let sy = ((fy + v * fh) / down - 0.5).clamp(self.lo.1, self.hi.1.max(self.lo.1));
                let (kx, ky) = (sx.floor(), sy.floor());
                let (ax, ay) = ((sx - kx) as f32, (sy - ky) as f32);
                let (kx, ky) = (kx as usize, ky as usize);
                let (kx1, ky1) = ((kx + 1).min(cols - 1), (ky + 1).min(rows - 1));
                let top = read(kx, ky) * (1.0 - ax) + read(kx1, ky) * ax;
                let bottom = read(kx, ky1) * (1.0 - ax) + read(kx1, ky1) * ax;
                sum += top * (1.0 - ay) + bottom * ay;
            }
        }
        Some((sum / (tx * ty) as f32, (cover * self.level) as f32))
    }
}
