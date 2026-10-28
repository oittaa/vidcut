use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use vidcut_core::fingerprint::Origin;
use vidcut_core::model::{ColorSpace, Hdr, LightLevel};
use vidcut_core::render::{Image, VideoPiece};
use vidcut_core::time::Rational;

use crate::decode::{Input, VideoReader};
use crate::error::{Error, Result};
use crate::ffmpeg::{format, frame};

pub const PERCENTAGES: [u8; 9] = [1, 5, 10, 25, 50, 75, 90, 95, 99];

const SDR_WHITE: f32 = 203.0;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameLight {
    pub max: f32,
    pub average: f32,
    pub maxscl: [f32; 3],
    pub percentiles: [f32; 9],
}

impl FrameLight {
    pub fn hdr10plus(&self) -> crate::hdr::Hdr10Plus {
        crate::hdr::Hdr10Plus {
            targeted_max_luminance: 0,
            maxscl: self.maxscl.map(f64::from),
            average_maxrgb: f64::from(self.average),
            percentiles: PERCENTAGES
                .iter()
                .zip(self.percentiles)
                .map(|(&p, v)| (p, f64::from(v)))
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LightAnalysis {
    pub frames: Vec<(f64, FrameLight)>,
    pub spans: Vec<(f64, f64)>,
}

const EPS: f64 = 1e-6;

impl LightAnalysis {
    pub fn covers(&self, from: f64, to: f64) -> bool {
        self.spans
            .iter()
            .any(|&(a, b)| a <= from + EPS && b >= to - EPS)
    }

    pub fn level(&self, sampled: impl IntoIterator<Item = f64>) -> Option<LightLevel> {
        let last = self.frames.len().checked_sub(1)?;
        let mut out: Option<(f32, f32)> = None;
        let mut i = 0;
        for t in sampled {
            i += self.frames[i..].partition_point(|(f, _)| *f < t - EPS);
            let l = &self.frames[i.min(last)].1;
            let (m, a) = out.get_or_insert((0.0, 0.0));
            *m = m.max(l.max);
            *a = a.max(l.average);
        }
        out.map(|(m, a)| {
            let max_cll = (m.ceil() as u32).max(1);
            LightLevel {
                max_cll,
                max_fall: (a.ceil() as u32).clamp(1, max_cll),
            }
        })
    }

    pub fn at(&self, t: f64) -> Option<&FrameLight> {
        let i = self.frames.partition_point(|(f, _)| *f <= t + EPS);
        self.frames.get(i.saturating_sub(1)).map(|(_, l)| l)
    }

    fn add(&mut self, from: f64, to: f64, frames: Vec<(f64, FrameLight)>) {
        let mut all = frames;
        all.append(&mut self.frames);
        all.sort_by(|a, b| a.0.total_cmp(&b.0));
        all.dedup_by(|b, a| (b.0 - a.0).abs() < EPS);
        self.frames = all;
        self.spans.push((from, to));
        self.spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for (a, b) in self.spans.drain(..) {
            match merged.last_mut() {
                Some(last) if a <= last.1 + EPS => last.1 = last.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        self.spans = merged;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub input: Input,
    pub from: f64,
    pub to: f64,
    pub color_space: ColorSpace,
}

impl Span {
    pub fn of(piece: &VideoPiece, fps: Rational) -> Option<Self> {
        let VideoPiece::Source {
            path,
            origin,
            frames,
            image,
            color_space,
            ..
        } = piece
        else {
            return None;
        };
        let input = crate::export::video::input_of(path, image.as_ref());
        if matches!(image, Some(Image::Still)) {
            return Some(Self {
                input,
                from: f64::NEG_INFINITY,
                to: f64::INFINITY,
                color_space: *color_space,
            });
        }
        let target = |pos| crate::export::video::target(fps, *origin, pos);
        let (a, b) = (piece.position(0), piece.position(frames - 1));
        Some(Self {
            input,
            from: target(a.min(b)),
            to: target(a.max(b) + 1),
            color_space: *color_space,
        })
    }
}

pub fn sampled(piece: &VideoPiece, fps: Rational) -> impl Iterator<Item = f64> + use<> {
    let (origin, n) = match piece {
        VideoPiece::Source {
            image: Some(Image::Still),
            ..
        } => (f64::NEG_INFINITY, 1),
        VideoPiece::Source { origin, frames, .. } => (*origin, *frames),
        VideoPiece::Black { .. } | VideoPiece::Mix { .. } | VideoPiece::Titled { .. } => (0.0, 0),
    };
    let still = origin == f64::NEG_INFINITY;
    let piece = piece.clone();
    (0..n).map(move |k| {
        let pos = if still { 0 } else { piece.position(k) };
        crate::export::video::target(fps, origin, pos)
    })
}

fn table(hdr: Option<Hdr>) -> Arc<Vec<f32>> {
    type Tables = Mutex<HashMap<Option<Hdr>, Arc<Vec<f32>>>>;
    static TABLES: OnceLock<Tables> = OnceLock::new();
    let mut tables = TABLES.get_or_init(Default::default).lock().unwrap();
    tables
        .entry(hdr)
        .or_insert_with(|| {
            Arc::new(
                (0..LUT)
                    .map(|i| {
                        let c = i as f64 / (LUT - 1) as f64;
                        (match hdr {
                            Some(Hdr::Pq) => {
                                let p = c.powf(1.0 / 78.843_75);
                                let num = (p - 0.835_937_5).max(0.0);
                                let den = 18.851_562_5 - 18.687_5 * p;
                                (num / den).powf(1.0 / 0.159_301_757_812_5) * 10_000.0
                            }
                            Some(Hdr::Hlg) => {
                                let e = if c <= 0.5 {
                                    c * c / 3.0
                                } else {
                                    ((c - 0.559_910_73) / 0.178_832_77).exp() / 12.0
                                        + 0.284_668_92 / 12.0
                                };
                                e.powf(1.2) * 1000.0
                            }
                            None => c.powf(2.4),
                        }) as f32
                    })
                    .collect(),
            )
        })
        .clone()
}

const LUT: usize = 4096;

fn lut(t: &[f32], c: f64) -> f32 {
    t[(c.clamp(0.0, 1.0) * (LUT - 1) as f64 + 0.5) as usize]
}

fn lowest(mut lo: usize, mut hi: usize, f: impl Fn(usize) -> bool) -> usize {
    while lo < hi {
        let m = lo + (hi - lo) / 2;
        if f(m) {
            hi = m;
        } else {
            lo = m + 1;
        }
    }
    lo
}

fn bin(nits: f32) -> usize {
    if nits < 100.0 {
        (nits.max(0.0) * 10.0 + 0.5) as usize
    } else {
        (900.0 + nits.min(10_000.0) + 0.5) as usize
    }
}

fn bin_nits(i: usize) -> f32 {
    if i < 1000 {
        i as f32 / 10.0
    } else {
        i as f32 - 900.0
    }
}

struct Samples<'a> {
    w: usize,
    h: usize,
    planes: [&'a [u8]; 3],
    rows: [usize; 3],
    wide: bool,
    semi: bool,
}

impl Samples<'_> {
    fn at(&self, plane: usize, i: usize) -> u16 {
        let data = self.planes[plane];
        if self.wide {
            u16::from_le_bytes([data[2 * i], data[2 * i + 1]]) >> 6
        } else {
            u16::from(data[i])
        }
    }

    fn luma(&self, x: usize, y: usize) -> u16 {
        self.at(0, y * self.rows[0] + x)
    }

    fn chroma(&self, cx: usize, cy: usize) -> (u16, u16) {
        if self.semi {
            let i = cy * self.rows[1] + 2 * cx;
            (self.at(1, i), self.at(1, i + 1))
        } else {
            (
                self.at(1, cy * self.rows[1] + cx),
                self.at(2, cy * self.rows[2] + cx),
            )
        }
    }

    fn tops(&self, cy: usize, top: &mut [u16]) {
        let w = self.w;
        top.fill(0);
        let unit = 1 + usize::from(self.wide);
        for y in [2 * cy, (2 * cy + 1).min(self.h - 1)] {
            let row = &self.planes[0][y * self.rows[0] * unit..][..w * unit];
            if self.wide {
                let (pairs, rest) = row.as_chunks::<4>();
                for (t, p) in top.iter_mut().zip(pairs) {
                    let a = u16::from_le_bytes([p[0], p[1]]);
                    let b = u16::from_le_bytes([p[2], p[3]]);
                    *t = (*t).max(a.max(b));
                }
                if let [lo, hi] = *rest {
                    top[w / 2] = top[w / 2].max(u16::from_le_bytes([lo, hi]));
                }
            } else {
                let (pairs, rest) = row.as_chunks::<2>();
                for (t, p) in top.iter_mut().zip(pairs) {
                    *t = (*t).max(u16::from(p[0].max(p[1])));
                }
                if let [last] = *rest {
                    top[w / 2] = top[w / 2].max(u16::from(last));
                }
            }
        }
        if self.wide {
            for t in top.iter_mut() {
                *t >>= 6;
            }
        }
    }

    fn chroma_range(&self, cy: usize) -> [(u16, u16); 2] {
        let cw = self.w.div_ceil(2);
        let mut out = [(u16::MAX, 0); 2];
        let mut see = |k: usize, v: u16| out[k] = (out[k].0.min(v), out[k].1.max(v));
        match (self.semi, self.wide) {
            (true, true) => {
                let row = &self.planes[1][cy * self.rows[1] * 2..][..4 * cw];
                for p in row.as_chunks::<4>().0 {
                    see(0, u16::from_le_bytes([p[0], p[1]]));
                    see(1, u16::from_le_bytes([p[2], p[3]]));
                }
            }
            (true, false) => {
                let row = &self.planes[1][cy * self.rows[1]..][..2 * cw];
                for p in row.as_chunks::<2>().0 {
                    see(0, u16::from(p[0]));
                    see(1, u16::from(p[1]));
                }
            }
            (false, _) => {
                for k in 0..2 {
                    for &v in &self.planes[1 + k][cy * self.rows[1 + k]..][..cw] {
                        see(k, u16::from(v));
                    }
                }
            }
        }
        if self.wide {
            out = out.map(|(lo, hi)| (lo >> 6, hi >> 6));
        }
        out
    }
}

struct Reading {
    table: Arc<Vec<f32>>,
    gamut: Option<[[f64; 3]; 3]>,
    y0: f64,
    to_y: f64,
    mid: f64,
    to_c: f64,
    kr: f64,
    kb: f64,
    cr_r: f64,
    cb_b: f64,
    to_g: f64,
}

impl Reading {
    fn of(c: &crate::color::PqLight, bits: u32) -> Self {
        let max_code = f64::from((1u32 << bits) - 1);
        let (y0, ys, cs) = if c.full_range {
            (0.0, max_code, max_code / 2.0)
        } else {
            let k = f64::from(1u32 << (bits - 8));
            (16.0 * k, 219.0 * k, 112.0 * k)
        };
        Self {
            table: table(c.hdr),
            gamut: c.hdr.is_none().then_some(c.gamut),
            y0,
            to_y: 1.0 / ys,
            mid: f64::from(1u32 << (bits - 1)),
            to_c: 1.0 / (2.0 * cs),
            kr: c.kr,
            kb: c.kb,
            cr_r: 2.0 * (1.0 - c.kr),
            cb_b: 2.0 * (1.0 - c.kb),
            to_g: 1.0 / (1.0 - c.kr - c.kb),
        }
    }

    fn red(&self, cr: u16) -> f64 {
        self.cr_r * ((f64::from(cr) - self.mid) * self.to_c)
    }

    fn blue(&self, cb: u16) -> f64 {
        self.cb_b * ((f64::from(cb) - self.mid) * self.to_c)
    }

    fn less_green(&self, r: f64, b: f64) -> f64 {
        (self.kr * r + self.kb * b) * self.to_g
    }

    fn codes(&self, y: u16, cb: u16, cr: u16) -> [f64; 3] {
        let yv = (f64::from(y) - self.y0) * self.to_y;
        let (r, b) = (self.red(cr), self.blue(cb));
        [yv + r, yv - self.less_green(r, b), yv + b]
    }

    fn key(&self) -> [u64; 7] {
        [
            self.mid, self.to_c, self.cr_r, self.cb_b, self.kr, self.kb, self.to_g,
        ]
        .map(f64::to_bits)
    }

    fn light(&self, [r, g, b]: [f64; 3]) -> [f32; 3] {
        let t = &self.table;
        let mut rgb = [lut(t, r), lut(t, g), lut(t, b)];
        if let Some(m) = &self.gamut {
            let l = rgb.map(f64::from);
            for (i, row) in m.iter().enumerate() {
                rgb[i] =
                    ((row[0] * l[0] + row[1] * l[1] + row[2] * l[2]).max(0.0) as f32) * SDR_WHITE;
            }
        }
        rgb
    }
}

fn peak(s: &Samples, r: &Reading) -> [f32; 3] {
    let hdr = r.gamut.is_none();
    let mut most = [f64::NEG_INFINITY; 3];
    let mut maxscl = [0f32; 3];
    let first_passing = |maxscl: [f32; 3]| {
        lowest(0, LUT, |e| {
            let l = r.light([e as f64 / (LUT - 1) as f64; 3]);
            (0..3).any(|i| l[i] > maxscl[i])
        }) as f64
    };
    let mut first = first_passing(maxscl);
    let mut top = vec![0u16; s.w.div_ceil(2)];
    for cy in 0..s.h.div_ceil(2) {
        s.tops(cy, &mut top);
        let [(cb_lo, cb_hi), (cr_lo, cr_hi)] = s.chroma_range(cy);
        let may_add = |y: usize| {
            let y = y as u16;
            let (hi, lo) = (r.codes(y, cb_hi, cr_hi), r.codes(y, cb_lo, cr_lo));
            let bound = [hi[0], lo[1], hi[2]];
            if hdr {
                (0..3).any(|i| bound[i] > most[i])
            } else {
                let l = r.light(bound);
                (0..3).any(|i| l[i] > maxscl[i])
            }
        };
        let brightest = usize::from(top.iter().copied().max().unwrap_or(0));
        if !may_add(brightest) {
            continue;
        }
        let from = lowest(0, brightest, may_add) as u16;
        for (cx, &y) in top.iter().enumerate() {
            if y < from {
                continue;
            }
            let (cb, cr) = s.chroma(cx, cy);
            let code = r.codes(y, cb, cr);
            if hdr {
                for i in 0..3 {
                    if code[i] > most[i] {
                        most[i] = code[i];
                    }
                }
            } else if code[0].max(code[1]).max(code[2]) * (LUT - 1) as f64 + 0.5 >= first {
                let l = r.light(code);
                if (0..3).any(|i| l[i] > maxscl[i]) {
                    maxscl = std::array::from_fn(|i| maxscl[i].max(l[i]));
                    first = first_passing(maxscl);
                }
            }
        }
    }
    if hdr { r.light(most) } else { maxscl }
}

fn readable(p: format::Pixel) -> bool {
    use format::Pixel as P;
    matches!(p, P::P010LE | P::NV12 | P::YUV420P | P::YUVJ420P)
}

fn reading(f: &frame::Video) -> Result<(Samples<'_>, Reading)> {
    let (bits, semi) = match f.format() {
        format::Pixel::P010LE => (10, true),
        format::Pixel::NV12 => (8, true),
        format::Pixel::YUV420P | format::Pixel::YUVJ420P => (8, false),
        other => {
            return Err(Error::Invalid(format!(
                "measuring light in {other:?} pictures"
            )));
        }
    };
    let planes = if semi { 2 } else { 3 };
    let unit = if bits > 8 { 2 } else { 1 };
    let s = Samples {
        w: f.width() as usize,
        h: f.height() as usize,
        planes: std::array::from_fn(|p| if p < planes { f.data(p) } else { &[] }),
        rows: std::array::from_fn(|p| if p < planes { f.stride(p) / unit } else { 0 }),
        wide: bits > 8,
        semi,
    };
    Ok((s, Reading::of(&crate::color::pq_light(f), bits)))
}

fn step_of(w: usize, h: usize) -> usize {
    let step = (((w * h) as f64 / 500_000.0).sqrt().ceil() as usize).max(1);
    if step > 1 { step + step % 2 } else { 1 }
}

struct Spread {
    hist: Vec<u32>,
    sum: f64,
    n: u64,
}

impl Spread {
    fn new() -> Self {
        Self {
            hist: vec![0u32; bin(10_000.0) + 1],
            sum: 0.0,
            n: 0,
        }
    }

    fn add(&mut self, r: &Reading, y: u16, cb: u16, cr: u16) {
        let rgb = r.light(r.codes(y, cb, cr));
        let m = rgb[0].max(rgb[1]).max(rgb[2]);
        self.sum += f64::from(m);
        self.n += 1;
        self.hist[bin(m)] += 1;
    }

    fn light(self, maxscl: [f32; 3]) -> FrameLight {
        let mut percentiles = [0f32; 9];
        for (p, pct) in percentiles.iter_mut().zip(PERCENTAGES) {
            let need = (self.n as f64 * f64::from(pct) / 100.0).ceil() as u64;
            let mut seen = 0u64;
            for (i, &count) in self.hist.iter().enumerate() {
                seen += u64::from(count);
                if seen >= need.max(1) {
                    *p = bin_nits(i);
                    break;
                }
            }
        }
        FrameLight {
            max: maxscl[0].max(maxscl[1]).max(maxscl[2]),
            average: (self.sum / self.n.max(1) as f64) as f32,
            maxscl,
            percentiles,
        }
    }
}

fn measure_with(
    f: &frame::Video,
    relayout: &mut crate::export::normalize::Relayout,
) -> Result<FrameLight> {
    let f = relayout.run(f, crate::decode::Reading::Stream)?;
    let (s, r) = reading(&f)?;
    let maxscl = peak(&s, &r);
    let (w, h) = (s.w, s.h);
    let step = step_of(w, h);
    let mut spread = Spread::new();
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            let (cb, cr) = s.chroma(x / 2, y / 2);
            spread.add(&r, s.luma(x, y), cb, cr);
        }
    }
    Ok(spread.light(maxscl))
}

struct Ranks {
    rank: Arc<[u32]>,
    pairs: Vec<u32>,
}

fn ranks(r: &Reading, bits: u32) -> Arc<Ranks> {
    type Made = Mutex<HashMap<[u64; 7], Arc<Ranks>>>;
    static MADE: OnceLock<Made> = OnceLock::new();
    let mut made = MADE.get_or_init(Default::default).lock().unwrap();
    made.entry(r.key())
        .or_insert_with(|| {
            let n = 1u16 << bits;
            let reds: Vec<f64> = (0..n).map(|cr| r.red(cr)).collect();
            let blues: Vec<f64> = (0..n).map(|cb| r.blue(cb)).collect();
            let mut less: Vec<(f64, u32)> = (0..u32::from(n) * u32::from(n))
                .map(|p| {
                    let (cb, cr) = ((p >> bits) as usize, (p & u32::from(n - 1)) as usize);
                    (r.less_green(reds[cr], blues[cb]), p)
                })
                .collect();
            less.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
            let mut rank = vec![0u32; less.len()];
            for (i, &(_, p)) in less.iter().enumerate() {
                rank[p as usize] = i as u32;
            }
            Arc::new(Ranks {
                rank: rank.into(),
                pairs: less.into_iter().map(|(_, p)| p).collect(),
            })
        })
        .clone()
}

struct Read {
    readout: crate::screen::light::Readout,
    reading: Reading,
    ranks: Arc<Ranks>,
    bits: u32,
}

impl Read {
    fn light(self) -> FrameLight {
        let (r, peaks) = (&self.reading, &self.readout.peaks);
        let stride = crate::screen::light::STRIDE;
        let n = 1u16 << self.bits;
        let mut most = [f64::NEG_INFINITY; 3];
        for c in 0..n {
            let i = usize::from(c);
            if let Some(y) = peaks[i].checked_sub(1) {
                most[0] = most[0].max(r.codes(y as u16, 0, c)[0]);
            }
            if let Some(y) = peaks[stride + i].checked_sub(1) {
                most[2] = most[2].max(r.codes(y as u16, c, 0)[2]);
            }
            if let Some(&p) = self.ranks.pairs.get(peaks[2 * stride + i] as usize) {
                let (cb, cr) = ((p >> self.bits) as u16, (p & u32::from(n - 1)) as u16);
                most[1] = most[1].max(r.codes(c, cb, cr)[1]);
            }
        }
        let mut spread = Spread::new();
        for &p in &self.readout.samples {
            let at = |shift: u32| ((p >> shift) & 1023) as u16;
            spread.add(r, at(0), at(10), at(20));
        }
        spread.light(r.light(most))
    }
}

#[derive(Default)]
struct OnGpu {
    meter: Option<(
        &'static crate::screen::Offscreen,
        crate::screen::light::Meter,
    )>,
    failed: bool,
    told: Option<bool>,
}

impl OnGpu {
    fn read(&mut self, f: &frame::Video) -> Option<Read> {
        let read = self.read_on_gpu(f);
        if self.told != Some(read.is_some()) {
            self.told = Some(read.is_some());
            let on = if read.is_some() { "GPU" } else { "CPU" };
            crate::debug!(
                "decoding",
                "the light of the pictures is measured on the {on}"
            );
        }
        read
    }

    fn read_on_gpu(&mut self, f: &frame::Video) -> Option<Read> {
        if self.failed || !crate::hw::on_gpu(f) {
            return None;
        }
        let bits = crate::screen::light::bits(f)?;
        let reading = Reading::of(&crate::color::pq_light(f), bits);
        if reading.gamut.is_some() {
            return None;
        }
        let on = crate::screen::offscreen().ok()?;
        if self
            .meter
            .as_ref()
            .is_some_and(|(was, _)| !std::ptr::eq(*was, on))
        {
            self.meter = None;
        }
        let (_, meter) = self.meter.get_or_insert_with(|| (on, Default::default()));
        let gpu = &on.gpu;
        if !meter.takes(gpu, f) {
            return None;
        }
        let step = step_of(f.width() as usize, f.height() as usize) as u32;
        let ranks = ranks(&reading, bits);
        match meter.measure(gpu, f, step, &ranks.rank) {
            Ok(readout) => Some(Read {
                readout,
                reading,
                ranks,
                bits,
            }),
            Err(e) => {
                eprintln!("vidcut: the light of HDR pictures is measured on the CPU: {e}");
                self.failed = true;
                self.meter = None;
                None
            }
        }
    }
}

enum Taken {
    Frame(frame::Video),
    Read(Box<Read>),
}

fn measure(
    span: &Span,
    decoder: Option<&crate::hw::GpuDecoder>,
    cancel: Option<&AtomicBool>,
    progress: &mut dyn FnMut(f64),
) -> Result<(Vec<(f64, FrameLight)>, bool)> {
    crate::init();
    let mut reader = match decoder {
        Some(gpu) => VideoReader::open_gpu_or_cpu(&span.input, gpu)?,
        None => VideoReader::open_input(&span.input)?,
    };
    reader.set_color_space(span.color_space);
    let tb = reader.time_base();
    let tick = f64::from(tb.numerator()) / f64::from(tb.denominator());
    if span.from.is_finite()
        && reader
            .first_pts()
            .is_some_and(|f| (f as f64 * tick) < span.from - EPS)
    {
        reader.seek(((span.from - EPS) / tick).floor() as i64)?;
    }
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 4));
    let (mut frames, ended) = on_threads(
        threads,
        || crate::export::normalize::Relayout::passing(readable),
        |give| {
            let mut gpu = OnGpu::default();
            while let Some(f) = reader.next_frame()? {
                if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                    return Err(Error::Invalid(crate::export::CANCELLED.into()));
                }
                let t = f.pts().unwrap_or(0) as f64 * tick;
                let taken = match gpu.read(&f) {
                    Some(read) => Taken::Read(Box::new(read)),
                    None => Taken::Frame(crate::decode::in_memory(f)?),
                };
                if !give((t, taken)) {
                    return Ok(false);
                }
                if t >= span.to - EPS {
                    return Ok(false);
                }
                if span.to.is_finite() && span.to > span.from {
                    progress(((t - span.from) / (span.to - span.from)).clamp(0.0, 1.0));
                }
            }
            Ok(true)
        },
        |relayout, (t, taken): (f64, Taken)| match taken {
            Taken::Frame(f) => Ok((t, measure_with(&f, relayout)?)),
            Taken::Read(read) => Ok((t, read.light())),
        },
    )?;
    frames.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok((frames, ended))
}

fn on_threads<T: Send, R: Send, S>(
    threads: usize,
    state: impl Fn() -> S + Sync,
    feed: impl FnOnce(&mut dyn FnMut(T) -> bool) -> Result<bool>,
    work: impl Fn(&mut S, T) -> Result<R> + Sync,
) -> Result<(Vec<R>, bool)> {
    let (to_work, inbox) = std::sync::mpsc::sync_channel::<T>(threads);
    let inbox = Mutex::new(inbox);
    let failed = AtomicBool::new(false);
    let job = crate::first::yields();
    std::thread::scope(|scope| {
        let working = (0..threads)
            .map(|i| {
                std::thread::Builder::new()
                    .name(format!("vidcut-light-{i}"))
                    .spawn_scoped(scope, || -> Result<Vec<R>> {
                        if job {
                            crate::first::as_job();
                        }
                        let mut own = state();
                        let mut made = Vec::new();
                        let mut error = None;
                        loop {
                            let next = inbox.lock().unwrap_or_else(|e| e.into_inner()).recv();
                            let Ok(item) = next else {
                                return error.map_or(Ok(made), Err);
                            };
                            if error.is_some() || failed.load(Ordering::Relaxed) {
                                continue;
                            }
                            match work(&mut own, item) {
                                Ok(r) => made.push(r),
                                Err(e) => {
                                    failed.store(true, Ordering::Relaxed);
                                    error = Some(e);
                                }
                            }
                        }
                    })
            })
            .collect::<std::io::Result<Vec<_>>>()?;
        let fed = feed(&mut |item| !failed.load(Ordering::Relaxed) && to_work.send(item).is_ok());
        drop(to_work);
        let mut made = Vec::new();
        for w in working {
            made.extend(w.join().expect("a working thread")?);
        }
        Ok((made, fed?))
    })
}

static CACHE: OnceLock<PathBuf> = OnceLock::new();

pub fn cache_in(dir: PathBuf) {
    let _ = CACHE.set(dir);
}

fn origin(input: &Input, space: ColorSpace) -> Origin {
    let path = input.path();
    let meta = std::fs::metadata(path).ok();
    let size = meta.as_ref().map_or(0, std::fs::Metadata::len);
    let mtime = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as i64);
    let origin = vidcut_core::fingerprint::origin(path, size, mtime);
    let origin = match input {
        Input::File(_) => origin,
        Input::Sequence {
            first,
            rate: (num, den),
            ..
        } => Origin {
            name: format!("{}-{first}-{num}-{den}", origin.name).into(),
            text: format!("{} {first} {num}/{den}", origin.text).into(),
        },
    };
    match space {
        ColorSpace::Tagged => origin,
        s => Origin {
            name: format!("{}-{}", origin.name, s.key()).into(),
            text: format!("{} {}", origin.text, s.key()).into(),
        },
    }
}

fn to_bytes(a: &LightAnalysis) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + a.spans.len() * 16 + a.frames.len() * 64);
    out.extend_from_slice(b"VCLIGHT2");
    out.extend_from_slice(&(a.spans.len() as u32).to_le_bytes());
    for (from, to) in &a.spans {
        out.extend_from_slice(&from.to_le_bytes());
        out.extend_from_slice(&to.to_le_bytes());
    }
    for (t, l) in &a.frames {
        out.extend_from_slice(&t.to_le_bytes());
        for v in [l.max, l.average]
            .iter()
            .chain(&l.maxscl)
            .chain(&l.percentiles)
        {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

fn from_bytes(b: &[u8]) -> Option<LightAnalysis> {
    let rest = b.strip_prefix(b"VCLIGHT2")?;
    let f64_at = |b: &[u8], i: usize| -> Option<f64> {
        Some(f64::from_le_bytes(b.get(i..i + 8)?.try_into().ok()?))
    };
    let count = u32::from_le_bytes(rest.get(..4)?.try_into().ok()?) as usize;
    let spans = (0..count)
        .map(|i| Some((f64_at(rest, 4 + i * 16)?, f64_at(rest, 12 + i * 16)?)))
        .collect::<Option<Vec<_>>>()?;
    let rest = rest.get(4 + count * 16..)?;
    const ENTRY: usize = 8 + 14 * 4;
    let (entries, []) = rest.as_chunks::<ENTRY>() else {
        return None;
    };
    let frames = entries
        .iter()
        .map(|e| {
            let t = f64::from_le_bytes(e[..8].try_into().expect("8"));
            let v: Vec<f32> = e[8..]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect();
            (
                t,
                FrameLight {
                    max: v[0],
                    average: v[1],
                    maxscl: [v[2], v[3], v[4]],
                    percentiles: v[5..14].try_into().expect("9"),
                },
            )
        })
        .collect();
    Some(LightAnalysis { frames, spans })
}

pub fn analysis(
    span: &Span,
    measure_now: bool,
    decoder: Option<&crate::hw::GpuDecoder>,
    cancel: Option<&AtomicBool>,
    progress: &mut dyn FnMut(f64),
) -> Result<Option<Arc<LightAnalysis>>> {
    static MEMORY: OnceLock<Mutex<HashMap<Origin, Arc<LightAnalysis>>>> = OnceLock::new();
    let origin = origin(&span.input, span.color_space);
    let memory = MEMORY.get_or_init(Default::default);
    let file = CACHE
        .get()
        .map(|d| d.join(format!("light-{}", origin.name)));
    let known = {
        let mut m = memory.lock().unwrap();
        if !m.contains_key(&origin)
            && let Some(a) = file
                .as_ref()
                .and_then(|f| vidcut_core::disk_cache::read(f))
                .and_then(|b| from_bytes(origin.check(&b)?))
        {
            m.insert(origin.clone(), Arc::new(a));
        }
        m.get(&origin).cloned()
    };
    if let Some(a) = known.filter(|a| a.covers(span.from, span.to)) {
        return Ok(Some(a));
    }
    if !measure_now {
        return Ok(None);
    }
    let (frames, ended) = measure(span, decoder, cancel, progress)?;
    let to = if ended { f64::INFINITY } else { span.to };
    let a = {
        let mut m = memory.lock().unwrap();
        let mut a = m.get(&origin).map(|a| (**a).clone()).unwrap_or_default();
        a.add(span.from, to, frames);
        let a = Arc::new(a);
        m.insert(origin.clone(), a.clone());
        a
    };
    if let Some(f) = &file {
        let _ = vidcut_core::disk_cache::write(f, &origin.stamp(&to_bytes(&a)));
    }
    Ok(Some(a))
}

pub fn piece_level(
    piece: &VideoPiece,
    fps: Rational,
    measure_now: bool,
    decoder: Option<&crate::hw::GpuDecoder>,
    cancel: Option<&AtomicBool>,
    progress: &mut dyn FnMut(f64),
) -> Result<Option<LightLevel>> {
    let Some(span) = Span::of(piece, fps) else {
        return Ok(None);
    };
    let a = analysis(&span, measure_now, decoder, cancel, progress)?;
    Ok(a.and_then(|a| a.level(sampled(piece, fps))))
}

pub(crate) struct Meter(crate::export::normalize::Relayout, OnGpu);

impl Meter {
    pub(crate) fn new() -> Self {
        Self(
            crate::export::normalize::Relayout::passing(readable),
            OnGpu::default(),
        )
    }

    pub(crate) fn measure(&mut self, f: &frame::Video) -> Result<FrameLight> {
        if let Some(read) = self.1.read(f) {
            return Ok(read.light());
        }
        let f = if crate::hw::on_gpu(f) {
            crate::hw::to_read(f)?
        } else {
            crate::export::video::frame_ref(f)?
        };
        measure_with(&f, &mut self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sequences_measurement_is_kept_by_its_first_picture_and_rate() {
        let o = origin(
            &Input::Sequence {
                pattern: PathBuf::from("/media/f_%04d.exr"),
                first: 1,
                rate: (24, 1),
            },
            ColorSpace::Tagged,
        );
        assert_eq!(&*o.name, "b4484e138e19524d2c01555ddf04af59-0-0-1-24-1");
        assert_eq!(&*o.text, "0 0 /media/f_%04d.exr 1 24/1");
    }

    #[test]
    fn a_failing_thread_ends_the_work() {
        let feed = |give: &mut dyn FnMut(u32) -> bool| {
            for i in 0..1000 {
                if !give(i) {
                    return Ok(false);
                }
            }
            Ok(true)
        };
        let fail =
            |_: &mut (), _: u32| -> Result<u32> { Err(Error::Invalid("unmeasurable".into())) };
        assert!(on_threads(2, || (), feed, fail).is_err());
    }

    #[test]
    fn a_4_2_2_picture_is_measured_in_p010() {
        let prores = frame::Video::new(format::Pixel::YUV422P10LE, 64, 64);
        let mut relayout = crate::export::normalize::Relayout::passing(readable);
        let measured = relayout
            .run(&prores, crate::decode::Reading::Stream)
            .unwrap();
        assert_eq!(measured.format(), format::Pixel::P010LE);
    }
}
