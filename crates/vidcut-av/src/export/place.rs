use vidcut_core::blend::mixed;
use vidcut_core::model::{CompositeMode, Title};
use vidcut_core::place::{Placing, ahead, apply, chain, lettered};
use vidcut_core::render::{Placed, WHOLE};

use super::video::frame_ref;
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

    fn grid(&self) -> (u32, u32) {
        (self.log2w, self.log2h)
    }

    fn top(&self) -> f64 {
        f64::from((1u32 << self.depth) - 1)
    }

    fn size(&self, (w, h): (u32, u32)) -> (usize, usize) {
        (
            w.div_ceil(1 << self.log2w) as usize,
            h.div_ceil(1 << self.log2h) as usize,
        )
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

fn alpha_index(pixel: format::Pixel) -> Option<usize> {
    let desc = pixel.descriptor()?;
    // SAFETY: FFmpeg's static descriptor of the format.
    let d = unsafe { &*desc.as_ptr() };
    let alpha = d.flags & ffi::AV_PIX_FMT_FLAG_ALPHA as u64 != 0;
    alpha.then(|| usize::from(d.nb_components).saturating_sub(1))
}

fn colored(comps: &[Comp]) -> usize {
    if comps.len() >= 3 { 3 } else { 1 }
}

fn alike(under: &[Comp], over: &[Comp]) -> Result<()> {
    let n = colored(under);
    let same = over.len() >= n
        && under[..n]
            .iter()
            .zip(over)
            .all(|(a, b)| a.grid() == b.grid() && a.depth == b.depth);
    match same {
        true => Ok(()),
        false => Err(Error::Invalid(
            "pictures of different layouts can't be laid over each other".into(),
        )),
    }
}

fn mean(
    values: &[f32],
    width: usize,
    height: usize,
    (x0, x1): (usize, usize),
    (y0, y1): (usize, usize),
) -> f32 {
    let (x1, y1) = (x1.min(width).max(x0 + 1), y1.min(height).max(y0 + 1));
    let mut sum = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            sum += values[y.min(height - 1) * width + x.min(width - 1)];
        }
    }
    sum / ((x1 - x0) * (y1 - y0)) as f32
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

pub(crate) struct Canvas {
    like: frame::Video,
    comps: Vec<Comp>,
    values: Vec<Vec<f32>>,
}

impl Canvas {
    pub(crate) fn of(f: &frame::Video) -> Result<Self> {
        let comps = comps(f.format())?;
        let size = (f.width(), f.height());
        let values = comps
            .iter()
            .map(|c| {
                let (width, height) = c.size(size);
                let (data, stride) = (f.data(c.plane), f.stride(c.plane));
                let mut values = vec![0.0; width * height];
                banded(&mut values, width, 0..height, |y, row| {
                    for (x, v) in row.iter_mut().enumerate() {
                        *v = c.read(data, y * stride + x * c.step + c.offset);
                    }
                });
                values
            })
            .collect();
        Ok(Self {
            like: frame_ref(f)?,
            comps,
            values,
        })
    }

    pub(crate) fn size(&self) -> (u32, u32) {
        (self.like.width(), self.like.height())
    }

    pub(crate) fn frame(&self) -> frame::Video {
        let mut out = copy(&self.like);
        let size = self.size();
        for (c, values) in self.comps.iter().zip(&self.values) {
            let (width, height) = c.size(size);
            let stride = out.stride(c.plane);
            banded(out.data_mut(c.plane), stride, 0..height, |y, row| {
                for (x, v) in values[y * width..][..width].iter().enumerate() {
                    c.write(row, x * c.step + c.offset, *v);
                }
            });
        }
        out
    }

    fn holding(&self, values: Vec<Vec<f32>>) -> Result<Self> {
        Ok(Self {
            like: frame_ref(&self.like)?,
            comps: self.comps.clone(),
            values,
        })
    }

    fn matches(&self, other: &Canvas) -> Result<()> {
        if other.like.format() != self.like.format() || other.size() != self.size() {
            return Err(Error::Invalid(
                "pictures of different sizes or formats can't be mixed".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn covered(&self, over: &Canvas, weight: u32) -> Result<Self> {
        let size = self.size();
        if over.size() != size {
            return Err(Error::Invalid(
                "pictures of different sizes can't be laid over each other".into(),
            ));
        }
        alike(&self.comps, &over.comps)?;
        let level = weight as f32 / WHOLE as f32;
        let alpha = alpha_index(over.like.format()).map(|k| (over.comps[k], &over.values[k]));
        let own = alpha_index(self.like.format());
        let values = self
            .comps
            .iter()
            .zip(&self.values)
            .enumerate()
            .map(|(k, (c, under))| {
                if Some(k) == own || k >= colored(&self.comps) {
                    return under.clone();
                }
                let (width, _) = c.size(size);
                under
                    .iter()
                    .zip(&over.values[k])
                    .enumerate()
                    .map(|(i, (u, o))| {
                        let a = alpha.map_or(1.0, |(ac, av)| {
                            let (aw, ah) = ac.size(size);
                            let (x, y) = (i % width, i / width);
                            let xs = (x << c.log2w, (x + 1) << c.log2w);
                            let ys = (y << c.log2h, (y + 1) << c.log2h);
                            mean(av, aw, ah, xs, ys) / ac.top() as f32
                        });
                        u + (o - u) * level * a
                    })
                    .collect()
            })
            .collect();
        self.holding(values)
    }

    pub(crate) fn zipped(&self, other: &Canvas, f: impl Fn(f32, f32) -> f32) -> Result<Self> {
        self.matches(other)?;
        self.holding(
            self.values
                .iter()
                .zip(&other.values)
                .map(|(a, b)| a.iter().zip(b).map(|(a, b)| f(*a, *b)).collect())
                .collect(),
        )
    }

    pub(crate) fn picked(&self, other: &Canvas, takes: impl Fn([u32; 4]) -> bool) -> Result<Self> {
        self.matches(other)?;
        let size = self.size();
        self.holding(
            self.comps
                .iter()
                .zip(self.values.iter().zip(&other.values))
                .map(|(c, (a, b))| {
                    let (width, height) = c.size(size);
                    a.iter()
                        .zip(b)
                        .enumerate()
                        .map(|(i, (a, b))| {
                            let at = [i % width, i / width, width, height].map(|v| v as u32);
                            if takes(at) { *b } else { *a }
                        })
                        .collect()
                })
                .collect(),
        )
    }
}

fn banded<T: Send>(
    data: &mut [T],
    width: usize,
    rows: std::ops::Range<usize>,
    each: impl Fn(usize, &mut [T]) + Sync,
) {
    let start = (rows.start * width).min(data.len());
    let mut rest = &mut data[start..];
    std::thread::scope(|scope| {
        for band in bands(rows) {
            let (chunk, next) = rest.split_at_mut((band.len() * width).min(rest.len()));
            rest = next;
            let each = &each;
            scope.spawn(move || {
                for (k, y) in band.enumerate() {
                    each(y, &mut chunk[k * width..][..width]);
                }
            });
        }
    });
}

pub(crate) struct Layer<'a> {
    pub(crate) picture: &'a frame::Video,
    pub(crate) placing: &'a Placing,
    pub(crate) fitted: [f64; 4],
    pub(crate) weight: u32,
}

impl Layer<'_> {
    pub(crate) fn area(&self, size: (u32, u32)) -> Option<[u32; 4]> {
        let fits = |v: u32, most: u32| (v & !1).min(most);
        self.placing
            .bounds((f64::from(size.0), f64::from(size.1)))
            .map(|[x0, y0, x1, y1]| {
                [
                    fits(x0, size.0),
                    fits(y0, size.1),
                    fits(x1 + 1, size.0),
                    fits(y1 + 1, size.1),
                ]
            })
            .filter(|[x0, y0, x1, y1]| x1 > x0 && y1 > y0)
    }

    pub(crate) fn gpu_planes(
        &self,
        under: &frame::Video,
        out: (u32, u32),
        size: (u32, u32),
        area: [u32; 4],
    ) -> Result<Option<[GpuPlane; 2]>> {
        let (picture, placing) = (self.picture, self.placing);
        let sw = crate::hw::sw_format(picture);
        if placing.levels.is_empty()
            || placing.levels.len() > GPU_LEVELS
            || !matches!(sw, format::Pixel::NV12 | format::Pixel::P010LE)
        {
            return Ok(None);
        }
        let samples = super::mix::Samples::of(sw)?;
        let coding = Coding::of(under, &comps(sw)?);
        let mut levels = [GpuLevel::default(); GPU_LEVELS];
        for (to, l) in levels.iter_mut().zip(&placing.levels) {
            *to = GpuLevel {
                back: l.back.map(|v| v as f32),
                edge: l.edge.map(|v| v as f32),
                inner: l.soft.inner.map(|v| v as f32),
                reach: l.soft.reach.map(|v| v as f32),
                opacity: l.opacity as f32,
            };
        }
        let [fx, fy, fw, fh] = self.fitted;
        let [dux, duy, dvx, dvy] = placing.step();
        let taps = |a: f64, b: f64| (a.hypot(b).ceil() as u32).clamp(1, 8);
        let source = |p: u32, q: u32| {
            let own = f64::from(1u32 << q);
            let ratio = f64::from(1u32 << p) / own;
            let (cols, rows) = (
                picture.width().div_ceil(1 << q),
                picture.height().div_ceil(1 << q),
            );
            GpuSource {
                cols,
                rows,
                tx: taps(dux * fw * ratio, dvx * fh * ratio),
                ty: taps(duy * fw * ratio, dvy * fh * ratio),
                sa: own as f32,
                sd: own as f32,
                lo: [(fx / own) as f32, (fy / own) as f32],
                hi: [
                    ((fx + fw) / own - 1.0).min(f64::from(cols) - 1.0) as f32,
                    ((fy + fh) / own - 1.0).min(f64::from(rows) - 1.0) as f32,
                ],
            }
        };
        let plane = |p: u32| {
            let step = (1u32 << p) as f32;
            GpuPlane {
                width: out.0.div_ceil(1 << p),
                height: out.1.div_ceil(1 << p),
                plane: p,
                bytes: samples.bytes as u32,
                shift: samples.shift,
                top: samples.top,
                x0: area[0] >> p,
                y0: area[1] >> p,
                x1: area[2].min(size.0).div_ceil(1 << p),
                y1: area[3].min(size.1).div_ceil(1 << p),
                count: placing.levels.len() as u32,
                mode: placing.mode.code(),
                full: u32::from(coding.full),
                frame: [out.0, out.1],
                across: step,
                down: step,
                level: (f64::from(self.weight) / f64::from(WHOLE)) as f32,
                kr: coding.kr as f32,
                kb: coding.kb as f32,
                unit: coding.unit as f32,
                fitted: self.fitted.map(|v| v as f32),
                sources: [source(p, 0), source(p, 1)],
                levels,
            }
        };
        Ok(Some([plane(0), plane(1)]))
    }
}

pub(crate) const GPU_LEVELS: usize = 8;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct GpuLevel {
    back: [f32; 8],
    edge: [f32; 4],
    inner: [f32; 4],
    reach: [f32; 2],
    opacity: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct GpuSource {
    cols: u32,
    rows: u32,
    tx: u32,
    ty: u32,
    sa: f32,
    sd: f32,
    lo: [f32; 2],
    hi: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct GpuPlane {
    pub(crate) width: u32,
    pub(crate) height: u32,
    plane: u32,
    bytes: u32,
    shift: u32,
    top: u32,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    count: u32,
    mode: u32,
    full: u32,
    frame: [u32; 2],
    across: f32,
    down: f32,
    level: f32,
    kr: f32,
    kb: f32,
    unit: f32,
    fitted: [f32; 4],
    sources: [GpuSource; 2],
    levels: [GpuLevel; GPU_LEVELS],
}

const _: () = assert!(std::mem::size_of::<GpuPlane>() == 788);

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

pub(crate) fn over(under: &Canvas, layer: &Layer) -> Result<Canvas> {
    let own = comps(layer.picture.format())?;
    alike(&under.comps, &own)?;
    let alpha = alpha_index(layer.picture.format()).map(|k| own[k]);
    let kept = alpha_index(under.like.format());
    let mut out = under.holding(under.values.clone())?;
    let size = under.size();
    let Some(area) = layer.placing.bounds((f64::from(size.0), f64::from(size.1))) else {
        return Ok(out);
    };
    let comps = &under.comps;
    let mode = layer.placing.mode;
    let colored = colored(comps);
    let coding = Coding::of(&under.like, &comps[..colored]);
    let below: Vec<Below> = (0..colored).map(|k| Below::of(under, k)).collect();
    for (i, (c, values)) in comps.iter().zip(&mut out.values).enumerate() {
        if Some(i) == kept || i >= own.len() {
            continue;
        }
        let m = Mixing {
            channel: i,
            own: Shading::of(own[i], layer, size, area, c.grid()),
            tops: match mode != CompositeMode::Normal && i < colored {
                true => own[..colored]
                    .iter()
                    .map(|k| Shading::of(*k, layer, size, area, c.grid()))
                    .collect(),
                false => Vec::new(),
            },
            alpha: alpha.map(|a| Shading::of(a, layer, size, area, c.grid())),
        };
        let (width, height) = c.size(size);
        let (m, coding, below) = (&m, &coding, &below);
        banded(values, width, m.own.rows(height), |y, row| {
            let s = &m.own;
            for x in s.columns() {
                let shown = match m.tops.is_empty() {
                    true => s.pixel(x, y),
                    false => m.mixed(mode, coding, below, s.comp.grid(), x, y),
                };
                if let Some((shown, a)) = shown {
                    let opaque = m.alpha.as_ref().map_or(1.0, |s| {
                        s.pixel(x, y).map_or(0.0, |(v, _)| v / s.comp.top() as f32)
                    });
                    row[x] += (shown - row[x]) * a * opaque;
                }
            }
        });
    }
    Ok(out)
}

struct Mixing<'a> {
    channel: usize,
    own: Shading<'a>,
    tops: Vec<Shading<'a>>,
    alpha: Option<Shading<'a>>,
}

impl Mixing<'_> {
    fn mixed(
        &self,
        mode: CompositeMode,
        coding: &Coding,
        below: &[Below],
        grid: (u32, u32),
        x: usize,
        y: usize,
    ) -> Option<(f32, f32)> {
        let (first, a) = self.tops[0].pixel(x, y)?;
        let mut top = [f64::from(first); 3];
        for (k, s) in self.tops.iter().enumerate().skip(1) {
            top[k] = f64::from(s.pixel(x, y)?.0);
        }
        let mut under = [0.0; 3];
        for (k, b) in below.iter().enumerate() {
            under[k] = f64::from(b.mean(grid, x, y));
        }
        if below.len() == 1 {
            under = [under[0]; 3];
        }
        let out = coding.codes(mixed(mode, coding.colors(top), coding.colors(under)));
        Some((out[self.channel] as f32, a))
    }
}

struct Below<'a> {
    comp: Comp,
    values: &'a [f32],
    size: (usize, usize),
}

impl<'a> Below<'a> {
    fn of(c: &'a Canvas, k: usize) -> Self {
        let comp = c.comps[k];
        Self {
            comp,
            values: &c.values[k],
            size: comp.size(c.size()),
        }
    }

    fn mean(&self, (gw, gh): (u32, u32), x: usize, y: usize) -> f32 {
        let span = |at: usize, g: u32, k: u32, n: usize| {
            let a = ((at << g) >> k).min(n - 1);
            a..(((at + 1) << g) >> k).clamp(a + 1, n)
        };
        let c = &self.comp;
        let (xs, ys) = (
            span(x, gw, c.log2w, self.size.0),
            span(y, gh, c.log2h, self.size.1),
        );
        let count = (xs.len() * ys.len()) as f32;
        let mut sum = 0.0;
        for j in ys {
            for i in xs.clone() {
                sum += self.values[j * self.size.0 + i];
            }
        }
        sum / count
    }
}

struct Coding {
    rgb: bool,
    gray: bool,
    full: bool,
    kr: f64,
    kb: f64,
    tops: [f64; 3],
    unit: f64,
}

impl Coding {
    fn of(f: &frame::Video, comps: &[Comp]) -> Self {
        let light = crate::color::pq_light(f);
        let tops = std::array::from_fn(|k| comps.get(k).unwrap_or(&comps[0]).top());
        Self {
            rgb: light.rgb,
            gray: comps.len() == 1,
            full: light.rgb || light.full_range,
            kr: light.kr,
            kb: light.kb,
            tops,
            unit: f64::from(1u32 << comps[0].depth.saturating_sub(8)),
        }
    }

    fn colors(&self, c: [f64; 3]) -> [f64; 3] {
        if self.rgb {
            return std::array::from_fn(|k| c[k] / self.tops[k]);
        }
        let (y, cb, cr) = match self.full {
            true => {
                let mid = (self.tops[0] + 1.0) / 2.0;
                (
                    c[0] / self.tops[0],
                    (c[1] - mid) / self.tops[1],
                    (c[2] - mid) / self.tops[2],
                )
            }
            false => {
                let u = self.unit;
                (
                    (c[0] - 16.0 * u) / (219.0 * u),
                    (c[1] - 128.0 * u) / (224.0 * u),
                    (c[2] - 128.0 * u) / (224.0 * u),
                )
            }
        };
        if self.gray {
            return [y; 3];
        }
        let (kr, kb) = (self.kr, self.kb);
        let r = y + 2.0 * (1.0 - kr) * cr;
        let b = y + 2.0 * (1.0 - kb) * cb;
        let g = (y - kr * r - kb * b) / (1.0 - kr - kb);
        [r, g, b]
    }

    fn codes(&self, [r, g, b]: [f64; 3]) -> [f64; 3] {
        if self.rgb {
            return [r * self.tops[0], g * self.tops[1], b * self.tops[2]];
        }
        let (kr, kb) = (self.kr, self.kb);
        let y = kr * r + (1.0 - kr - kb) * g + kb * b;
        let (cb, cr) = ((b - y) / (2.0 * (1.0 - kb)), (r - y) / (2.0 * (1.0 - kr)));
        match self.full {
            true => {
                let mid = (self.tops[0] + 1.0) / 2.0;
                [
                    y * self.tops[0],
                    cb * self.tops[1] + mid,
                    cr * self.tops[2] + mid,
                ]
            }
            false => {
                let u = self.unit;
                [
                    16.0 * u + 219.0 * u * y,
                    128.0 * u + 224.0 * u * cb,
                    128.0 * u + 224.0 * u * cr,
                ]
            }
        }
    }
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
    let [a, b, _, c, d, _, g, h] = map;
    let taps = |u: f64, v: f64| (u.hypot(v).ceil() as usize).clamp(1, 8);
    let (tx, ty) = (taps(a, c), taps(b, d));
    let shade = |x: u32, y: u32| -> [f32; 4] {
        let (cx, cy) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
        let Some((_, cover)) = placing.at(cx, cy) else {
            return [0.0; 4];
        };
        let w = g * cx + h * cy + 1.0;
        if !w.is_finite() || w.abs() < 1e-12 {
            return [0.0; 4];
        }
        let [qx, qy] = apply(&map, [cx, cy]);
        let (dx, dy) = ((a - qx * g) / w, (c - qy * g) / w);
        let (ex, ey) = ((b - qx * h) / w, (d - qy * h) / w);
        let mut sum = [0.0; 4];
        for j in 0..ty {
            for i in 0..tx {
                let ox = (i as f64 + 0.5) / tx as f64 - 0.5;
                let oy = (j as f64 + 0.5) / ty as f64 - 0.5;
                let p = bilinear(s, [qx + dx * ox + ex * oy, qy + dy * ox + ey * oy]);
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
    let own = comps(layer.picture.format())?;
    let comps = comps(pixel)?;
    alike(&comps, &own)?;
    let opaque = alpha_index(layer.picture.format()).map(|k| own[k]);
    let (w, h) = (area[2] - area[0], area[3] - area[1]);
    let mut color = frame::Video::new(pixel, w, h);
    let mut alpha = frame::Video::new(pixel, w, h);
    for f in [&mut color, &mut alpha] {
        for p in 0..f.planes() {
            f.data_mut(p).fill(0);
        }
    }
    for p in 0..color.planes() {
        let mine: Vec<(Comp, Shading, Option<Shading>)> = comps
            .iter()
            .zip(&own)
            .filter(|(c, _)| c.plane == p)
            .map(|(c, o)| {
                (
                    *c,
                    Shading::of(*o, layer, size, area, c.grid()),
                    opaque.map(|a| Shading::of(a, layer, size, area, c.grid())),
                )
            })
            .collect();
        let Some((first, _, _)) = mine.first() else {
            continue;
        };
        let top = (area[1] >> first.log2h) as usize;
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
                        for (c, s, opaque) in mine {
                            let left = (area[0] >> c.log2w) as usize;
                            let full = c.top() as f32;
                            for x in s.columns() {
                                if let Some((shown, a)) = s.pixel(x, y) {
                                    let opaque = opaque.as_ref().map_or(1.0, |o| {
                                        o.pixel(x, y).map_or(0.0, |(v, _)| v / o.comp.top() as f32)
                                    });
                                    let to = (x - left) * c.step + c.offset;
                                    c.write(crow, to, shown);
                                    c.write(arow, to, a * opaque * full);
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
    own: (f64, f64),
    grid: (usize, usize),
    span: (usize, usize),
    lines: (usize, usize),
    taps: (usize, usize),
    lo: (f64, f64),
    hi: (f64, f64),
}

impl<'a> Shading<'a> {
    fn of(
        comp: Comp,
        layer: &Layer<'a>,
        size: (u32, u32),
        area: [u32; 4],
        (gw, gh): (u32, u32),
    ) -> Self {
        let p = comp.plane;
        let (across, down) = (f64::from(1u32 << gw), f64::from(1u32 << gh));
        let (sa, sd) = (f64::from(1u32 << comp.log2w), f64::from(1u32 << comp.log2h));
        let picture = layer.picture;
        let grid = comp.size((picture.width(), picture.height()));
        let [fx, fy, fw, fh] = layer.fitted;
        let [dux, duy, dvx, dvy] = layer.placing.step();
        let taps = |a: f64, b: f64| (a.hypot(b).ceil() as usize).clamp(1, 8);
        let x0 = (area[0] >> gw) as usize;
        let x1 = area[2].min(size.0).div_ceil(1 << gw) as usize;
        Self {
            comp,
            data: picture.data(p),
            stride: picture.stride(p),
            placing: layer.placing,
            fitted: layer.fitted,
            level: f64::from(layer.weight) / f64::from(WHOLE),
            scale: (across, down),
            own: (sa, sd),
            grid,
            span: (x0, x1),
            lines: (
                (area[1] >> gh) as usize,
                area[3].min(size.1).div_ceil(1 << gh) as usize,
            ),
            taps: (
                taps(dux * fw * across / sa, dvx * fh * across / sd),
                taps(duy * fw * down / sa, dvy * fh * down / sd),
            ),
            lo: (fx / sa, fy / sd),
            hi: (
                ((fx + fw) / sa - 1.0).min(grid.0 as f64 - 1.0),
                ((fy + fh) / sd - 1.0).min(grid.1 as f64 - 1.0),
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
        let (sa, sd) = self.own;
        let (cx, cy) = ((x as f64 + 0.5) * across, (y as f64 + 0.5) * down);
        let (_, cover) = self.placing.at(cx, cy)?;
        let [fx, fy, fw, fh] = self.fitted;
        let back = self.placing.levels.last().map_or([0.0; 8], |l| l.back);
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
                let w = back[6] * px + back[7] * py + 1.0;
                if !w.is_finite() || w.abs() < 1e-12 {
                    continue;
                }
                let u = (back[0] * px + back[1] * py + back[2]) / w;
                let v = (back[3] * px + back[4] * py + back[5]) / w;
                let sx = ((fx + u * fw) / sa - 0.5).clamp(self.lo.0, self.hi.0.max(self.lo.0));
                let sy = ((fy + v * fh) / sd - 0.5).clamp(self.lo.1, self.hi.1.max(self.lo.1));
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
