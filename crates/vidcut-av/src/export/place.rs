use vidcut_core::place::Placing;
use vidcut_core::render::WHOLE;

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

pub(crate) fn over(
    under: &frame::Video,
    over: &frame::Video,
    placing: &Placing,
    fitted: [f64; 4],
    weight: u32,
) -> Result<frame::Video> {
    let pixel = under.format();
    if over.format() != pixel {
        return Err(Error::Invalid(
            "pictures of different formats can't be placed over each other".into(),
        ));
    }
    let comps = comps(pixel)?;
    let mut out = frame::Video::new(pixel, under.width(), under.height());
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
    let (w, h) = (under.width(), under.height());
    let Some(area) = placing.bounds((f64::from(w), f64::from(h))) else {
        return Ok(out);
    };
    let level = f64::from(weight) / f64::from(WHOLE);
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().clamp(1, 8));
    for p in 0..out.planes() {
        let mine: Vec<Comp> = comps.iter().copied().filter(|c| c.plane == p).collect();
        let Some(first) = mine.first().copied() else {
            continue;
        };
        let rows = (out.plane_height(p) as usize).min(h.div_ceil(1 << first.log2h) as usize);
        let y0 = (area[1] >> first.log2h) as usize;
        let y1 = (area[3].div_ceil(1 << first.log2h) as usize).min(rows);
        if y1 <= y0 {
            continue;
        }
        let stride = out.stride(p);
        let source = Source {
            data: over.data(p),
            stride: over.stride(p),
            size: (over.width(), over.height()),
            width: w,
        };
        let band = (y1 - y0).div_ceil(threads);
        let dst = &mut out.data_mut(p)[y0 * stride..];
        std::thread::scope(|scope| {
            for (k, chunk) in dst.chunks_mut(band * stride).enumerate() {
                let first_row = y0 + k * band;
                if first_row >= y1 {
                    break;
                }
                let last_row = (first_row + band).min(y1);
                let (mine, source) = (&mine, &source);
                scope.spawn(move || {
                    for y in first_row..last_row {
                        let row = &mut chunk[(y - first_row) * stride..][..stride];
                        for c in mine {
                            draw_row(row, y, c, source, placing, fitted, area, level);
                        }
                    }
                });
            }
        });
    }
    Ok(out)
}

struct Source<'a> {
    data: &'a [u8],
    stride: usize,
    size: (u32, u32),
    width: u32,
}

#[allow(clippy::too_many_arguments)]
fn draw_row(
    row: &mut [u8],
    y: usize,
    c: &Comp,
    source: &Source,
    placing: &Placing,
    [fx, fy, fw, fh]: [f64; 4],
    area: [u32; 4],
    level: f64,
) {
    let (across, down) = (f64::from(1u32 << c.log2w), f64::from(1u32 << c.log2h));
    let cols = source.size.0.div_ceil(1 << c.log2w) as usize;
    let rows = source.size.1.div_ceil(1 << c.log2h) as usize;
    let x0 = (area[0] >> c.log2w) as usize;
    let x1 = (area[2].div_ceil(1 << c.log2w)).min(source.width.div_ceil(1 << c.log2w)) as usize;
    let [dux, duy, dvx, dvy] = placing.step();
    let taps = |a: f64, b: f64| (a.hypot(b).ceil() as usize).clamp(1, 8);
    let tx = taps(dux * fw, dvx * fh * across / down);
    let ty = taps(duy * fw * down / across, dvy * fh);
    let back = placing.levels.last().map_or([0.0; 6], |l| l.back);
    let lo = (fx / across, fy / down);
    let hi = (
        ((fx + fw) / across - 1.0).min(cols as f64 - 1.0),
        ((fy + fh) / down - 1.0).min(rows as f64 - 1.0),
    );
    let at = |col: usize, line: usize| line * source.stride + col * c.step + c.offset;
    for x in x0..x1 {
        let (cx, cy) = ((x as f64 + 0.5) * across, (y as f64 + 0.5) * down);
        let Some((_, cover)) = placing.at(cx, cy) else {
            continue;
        };
        let mut sum = 0.0f32;
        for j in 0..ty {
            for i in 0..tx {
                let px = x as f64 * across + (i as f64 + 0.5) / tx as f64 * across;
                let py = y as f64 * down + (j as f64 + 0.5) / ty as f64 * down;
                let u = back[0] * px + back[1] * py + back[2];
                let v = back[3] * px + back[4] * py + back[5];
                let sx = ((fx + u * fw) / across - 0.5).clamp(lo.0, hi.0.max(lo.0));
                let sy = ((fy + v * fh) / down - 0.5).clamp(lo.1, hi.1.max(lo.1));
                let (kx, ky) = (sx.floor(), sy.floor());
                let (ax, ay) = ((sx - kx) as f32, (sy - ky) as f32);
                let (kx, ky) = (kx as usize, ky as usize);
                let (kx1, ky1) = ((kx + 1).min(cols - 1), (ky + 1).min(rows - 1));
                let read = |col, line| c.read(source.data, at(col, line));
                let top = read(kx, ky) * (1.0 - ax) + read(kx1, ky) * ax;
                let bottom = read(kx, ky1) * (1.0 - ax) + read(kx1, ky1) * ax;
                sum += top * (1.0 - ay) + bottom * ay;
            }
        }
        let shown = sum / (tx * ty) as f32;
        let a = (cover * level) as f32;
        let to = x * c.step + c.offset;
        let under = c.read(row, to);
        c.write(row, to, under + (shown - under) * a);
    }
}
