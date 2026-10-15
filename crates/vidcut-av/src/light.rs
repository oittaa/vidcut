//! Light measured in pictures as a PQ export shows them: each frame's
//! brightest pixel, its average and the distribution of the brightest
//! channel (maxRGB), in cd/m². For MaxCLL and MaxFALL where the sources say
//! none, and for HDR10+ made for frames without it (decision D46). What a
//! piece shows is measured once, every frame, and the result kept with the
//! rest of its file's (`cache_in`).
//!
//! Each pixel is read as the export converts it (`color::pq_light`): PQ's
//! EOTF, HLG's inverse OETF and 1.2 system gamma on a 1000-nit display, SDR
//! as BT.1886 with its white at 203 cd/m² in BT.2020. The peaks are of
//! every pixel, a highlight of one too; the average and the distribution
//! come from a grid of at most half a million pixels in each frame, every
//! pixel of smaller ones.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use vidcut_core::model::{Hdr, LightLevel};
use vidcut_core::render::{Image, VideoPiece};
use vidcut_core::time::Rational;

use crate::decode::{Input, VideoReader};
use crate::error::{Error, Result};
use crate::ffmpeg::{format, frame};

/// The maxRGB percentiles kept per frame, as HDR10+ carries them.
pub const PERCENTAGES: [u8; 9] = [1, 5, 10, 25, 50, 75, 90, 95, 99];

/// SDR white in PQ output, cd/m² (ITU-R BT.2408).
const SDR_WHITE: f32 = 203.0;

/// One frame's light in PQ output, cd/m².
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameLight {
    /// The brightest channel of the brightest pixel.
    pub max: f32,
    /// maxRGB averaged over the frame.
    pub average: f32,
    /// Each channel's brightest, red, green, blue (HDR10+'s MaxSCL).
    pub maxscl: [f32; 3],
    /// maxRGB at [`PERCENTAGES`].
    pub percentiles: [f32; 9],
}

impl FrameLight {
    /// As HDR10+, for a frame without it (decision D46).
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

/// What is measured of a file: frames' light in presentation order with
/// their times (seconds), and the stretches `[from, to)` whose frames are
/// all there, each with the first frame from `to` on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LightAnalysis {
    pub frames: Vec<(f64, FrameLight)>,
    pub spans: Vec<(f64, f64)>,
}

/// Times closer than this are the same.
const EPS: f64 = 1e-6;

impl LightAnalysis {
    /// Whether `[from, to)` is measured.
    pub fn covers(&self, from: f64, to: f64) -> bool {
        self.spans
            .iter()
            .any(|&(a, b)| a <= from + EPS && b >= to - EPS)
    }

    /// MaxCLL and MaxFALL of the frames shown at `sampled`, times (seconds)
    /// in order within what is measured: at each, the first frame from then
    /// on, or the last (the sampling rule, `export::video`). Frames between
    /// them, which a lower output rate skips, don't count. `None` without
    /// frames.
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

    /// The frame shown at `t` (seconds): the last one at or before it.
    pub fn at(&self, t: f64) -> Option<&FrameLight> {
        let i = self.frames.partition_point(|(f, _)| *f <= t + EPS);
        self.frames.get(i.saturating_sub(1)).map(|(_, l)| l)
    }

    /// With `frames`, measured over `[from, to)`.
    fn add(&mut self, from: f64, to: f64, frames: Vec<(f64, FrameLight)>) {
        let mut all = frames;
        all.append(&mut self.frames);
        // Stable: of two at one time, the new one stays.
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

/// What a piece shows: its pictures and their source times (seconds). A
/// mix's pictures each have their own (`VideoPiece::layers`).
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub input: Input,
    pub from: f64,
    pub to: f64,
}

impl Span {
    /// What `piece` shows at `fps` (the plan's), by the sampling rule
    /// (`export::video`); `None` for black. A still shows its one picture.
    pub fn of(piece: &VideoPiece, fps: Rational) -> Option<Self> {
        let VideoPiece::Source {
            path,
            origin,
            frames,
            image,
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
            });
        }
        let target = |pos| crate::export::video::target(fps, *origin, pos);
        // Its first and last frames, whichever way it walks its source.
        let (a, b) = (piece.position(0), piece.position(frames - 1));
        Some(Self {
            input,
            from: target(a.min(b)),
            to: target(a.max(b) + 1),
        })
    }
}

/// When the frames `piece` shows at `fps` are sampled, in order (the
/// sampling rule, `export::video`): each shows the first source frame from
/// then on, or the last. A still shows its one picture; black, none; a mix,
/// none of its own (its layers have theirs).
pub fn sampled(piece: &VideoPiece, fps: Rational) -> impl Iterator<Item = f64> + use<> {
    let (origin, n) = match piece {
        VideoPiece::Source {
            image: Some(Image::Still),
            ..
        } => (f64::NEG_INFINITY, 1),
        VideoPiece::Source { origin, frames, .. } => (*origin, *frames),
        VideoPiece::Black { .. } | VideoPiece::Mix { .. } => (0.0, 0),
    };
    let still = origin == f64::NEG_INFINITY;
    let piece = piece.clone();
    (0..n).map(move |k| {
        let pos = if still { 0 } else { piece.position(k) };
        crate::export::video::target(fps, origin, pos)
    })
}

/// A transfer's light per normalized code, cd/m² (SDR: relative to its
/// white, before the gamut).
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
    // Not negative after the clamp: a half added and cut off rounds.
    t[(c.clamp(0.0, 1.0) * (LUT - 1) as f64 + 0.5) as usize]
}

/// The lowest of `lo..hi` where `f` holds, given that it fails below that
/// and holds from it on; `hi` when nowhere.
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

/// The histogram bin of `nits`: 0.1 cd/m² wide up to 100 (as fine as
/// HDR10+ counts), 1 wide above, to 10000.
fn bin(nits: f32) -> usize {
    // Not negative: a half added and cut off rounds.
    if nits < 100.0 {
        (nits.max(0.0) * 10.0 + 0.5) as usize
    } else {
        (900.0 + nits.min(10_000.0) + 0.5) as usize
    }
}

/// The light of histogram bin `i`.
fn bin_nits(i: usize) -> f32 {
    if i < 1000 {
        i as f32 / 10.0
    } else {
        i as f32 - 900.0
    }
}

/// A 4:2:0 picture's codes: each pixel's luma and the chroma its 2×2 block
/// shares.
struct Samples<'a> {
    w: usize,
    h: usize,
    /// Y, then Cb and Cr (interleaved in the second with `semi`).
    planes: [&'a [u8]; 3],
    /// Each plane's row, in samples.
    rows: [usize; 3],
    /// Two bytes a sample, the code in the top ten bits (P010).
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

    /// Cb and Cr of block (`cx`, `cy`).
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

    /// The brightest luma code of each 2×2 block in block row `cy`, into
    /// `top` (a block per entry).
    fn tops(&self, cy: usize, top: &mut [u16]) {
        let w = self.w;
        top.fill(0);
        let unit = 1 + usize::from(self.wide);
        // An odd last row is its blocks' only one.
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

    /// The lowest and highest Cb, then Cr, of block row `cy`.
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

/// How a picture's codes read as light in PQ output.
struct Reading {
    table: Arc<Vec<f32>>,
    /// SDR's gamut into BT.2020; `None` for HDR, already in it.
    gamut: Option<[[f64; 3]; 3]>,
    /// Luma's black and scale, chroma's middle and scale.
    y0: f64,
    to_y: f64,
    mid: f64,
    to_c: f64,
    kr: f64,
    kb: f64,
    /// What Cr adds to R', Cb to B', and G' per what they take.
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

    /// R', G', B' of a pixel: its luma's, plus what the chroma adds to
    /// each. Each rises with the luma; R' with Cr, B' with Cb, G' against
    /// both.
    fn codes(&self, y: u16, cb: u16, cr: u16) -> [f64; 3] {
        let yv = (f64::from(y) - self.y0) * self.to_y;
        let r = self.cr_r * ((f64::from(cr) - self.mid) * self.to_c);
        let b = self.cb_b * ((f64::from(cb) - self.mid) * self.to_c);
        [yv + r, yv - (self.kr * r + self.kb * b) * self.to_g, yv + b]
    }

    /// The light of each channel, cd/m².
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

/// Each channel's brightest in the picture, of every pixel.
///
/// Each channel's light rises with its code: the transfers rise, and SDR's
/// gamut has no negative coefficient (every source's primaries lie within
/// BT.2020's). So in a 2×2 block, whose chroma is one, the brightest luma
/// gives the brightest channels; and no block of a row is brighter than
/// its luma with the row's chroma at its extremes. Only blocks whose luma
/// could pass what is found are read. Without a gamut (HDR), each
/// channel's light is of its own code alone: the highest codes are kept,
/// and their light found at the end.
fn peak(s: &Samples, r: &Reading) -> [f32; 3] {
    let hdr = r.gamut.is_none();
    let mut most = [f64::NEG_INFINITY; 3];
    let mut maxscl = [0f32; 3];
    // SDR: the first table entry that, in all three channels, would pass
    // `maxscl` in one; a block whose every code is short of it can't.
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

/// The layouts [`reading`] reads: 4:2:0 in 8 bits and P010. Pictures in
/// others are brought to them (`export::normalize::Relayout`).
fn readable(p: format::Pixel) -> bool {
    use format::Pixel as P;
    matches!(p, P::P010LE | P::NV12 | P::YUV420P | P::YUVJ420P)
}

/// `f`'s samples and how they read, for a picture in a layout it reads
/// ([`readable`]).
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

fn measure_with(
    f: &frame::Video,
    relayout: &mut crate::export::normalize::Relayout,
) -> Result<FrameLight> {
    let f = relayout.run(f)?;
    let (s, r) = reading(&f)?;
    let maxscl = peak(&s, &r);
    // The average and the distribution, of at most half a million pixels
    // on the chroma grid.
    let (w, h) = (s.w, s.h);
    let step = (((w * h) as f64 / 500_000.0).sqrt().ceil() as usize).max(1);
    let step = if step > 1 { step + step % 2 } else { 1 };
    let mut hist = vec![0u32; bin(10_000.0) + 1];
    let (mut sum, mut n) = (0f64, 0u64);
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            let (cb, cr) = s.chroma(x / 2, y / 2);
            let rgb = r.light(r.codes(s.luma(x, y), cb, cr));
            let m = rgb[0].max(rgb[1]).max(rgb[2]);
            sum += f64::from(m);
            n += 1;
            hist[bin(m)] += 1;
        }
    }
    let mut percentiles = [0f32; 9];
    for (p, pct) in percentiles.iter_mut().zip(PERCENTAGES) {
        let need = (n as f64 * f64::from(pct) / 100.0).ceil() as u64;
        let mut seen = 0u64;
        for (i, &count) in hist.iter().enumerate() {
            seen += u64::from(count);
            if seen >= need.max(1) {
                *p = bin_nits(i);
                break;
            }
        }
    }
    Ok(FrameLight {
        max: maxscl[0].max(maxscl[1]).max(maxscl[2]),
        average: (sum / n.max(1) as f64) as f32,
        maxscl,
        percentiles,
    })
}

/// Measures the frames of `span`, decoded on `decoder` where it decodes
/// the file (copied down whole) or on the CPU, and the first one from its end
/// on; and whether the file ended before that. `progress` hears the share
/// decoded. Threads of their own measure the frames while the next ones
/// decode.
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
            while let Some(f) = reader.next_frame()? {
                if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                    return Err(Error::Invalid(crate::export::CANCELLED.into()));
                }
                let t = f.pts().unwrap_or(0) as f64 * tick;
                // Measuring failed: its error follows.
                if !give((t, crate::decode::in_memory(f)?)) {
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
        |relayout, (t, f): (f64, frame::Video)| Ok((t, measure_with(&f, relayout)?)),
    )?;
    frames.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok((frames, ended))
}

/// Hands what `feed` gives to `threads` threads, which `work` it, each
/// with its own `state`. `give` says whether to go on: once a thread
/// fails, the others take what comes and drop it, so `feed` never waits
/// for them. What they made and what `feed` returned, or the first error.
fn on_threads<T: Send, R: Send, S>(
    threads: usize,
    state: impl Fn() -> S + Sync,
    feed: impl FnOnce(&mut dyn FnMut(T) -> bool) -> Result<bool>,
    work: impl Fn(&mut S, T) -> Result<R> + Sync,
) -> Result<(Vec<R>, bool)> {
    let (to_work, inbox) = std::sync::mpsc::sync_channel::<T>(threads);
    let inbox = Mutex::new(inbox);
    let failed = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let working: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| -> Result<Vec<R>> {
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
            .collect();
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

/// Keeps measurements in `dir` (`~/.cache/vidcut/analysis`); without it
/// they last only the process.
pub fn cache_in(dir: PathBuf) {
    let _ = CACHE.set(dir);
}

/// What names `input` as it is now: its path(s), size and time.
fn key(input: &Input) -> String {
    let path = input.path();
    let meta = std::fs::metadata(path).ok();
    let size = meta.as_ref().map_or(0, std::fs::Metadata::len);
    let mtime = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    let text = format!("{input:?}|{size}|{mtime}");
    format!("{:016x}", vidcut_core::hash64(text.as_bytes()))
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

/// The measurement of `span`'s file that covers it: kept in memory, or on
/// disk, or (with `measure_now`) made now on `decoder` and kept. `None`
/// when it isn't there and `measure_now` is false. `progress` hears the
/// share of `span` measured.
pub fn analysis(
    span: &Span,
    measure_now: bool,
    decoder: Option<&crate::hw::GpuDecoder>,
    cancel: Option<&AtomicBool>,
    progress: &mut dyn FnMut(f64),
) -> Result<Option<Arc<LightAnalysis>>> {
    static MEMORY: OnceLock<Mutex<HashMap<String, Arc<LightAnalysis>>>> = OnceLock::new();
    let k = key(&span.input);
    let memory = MEMORY.get_or_init(Default::default);
    let file = CACHE.get().map(|d| d.join(format!("light-{k}")));
    let known = {
        let mut m = memory.lock().unwrap();
        if !m.contains_key(&k)
            && let Some(a) = file
                .as_ref()
                .and_then(|f| std::fs::read(f).ok())
                .and_then(|b| from_bytes(&b))
        {
            m.insert(k.clone(), Arc::new(a));
        }
        m.get(&k).cloned()
    };
    if let Some(a) = known.filter(|a| a.covers(span.from, span.to)) {
        return Ok(Some(a));
    }
    if !measure_now {
        return Ok(None);
    }
    let (frames, ended) = measure(span, decoder, cancel, progress)?;
    let to = if ended { f64::INFINITY } else { span.to };
    // Added to what is known now: another measurement may have ended
    // meanwhile.
    let a = {
        let mut m = memory.lock().unwrap();
        let mut a = m.get(&k).map(|a| (**a).clone()).unwrap_or_default();
        a.add(span.from, to, frames);
        let a = Arc::new(a);
        m.insert(k, a.clone());
        a
    };
    if let Some(f) = &file {
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let part = f.with_extension(format!("part{}", std::process::id()));
        if std::fs::write(&part, to_bytes(&a)).is_ok() {
            let _ = std::fs::rename(&part, f);
        }
    }
    Ok(Some(a))
}

/// MaxCLL and MaxFALL of what `piece` shows at `fps`, from its file's
/// measurement (made now with `measure_now`); `None` for black or when it
/// isn't measured.
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

/// Measures single frames as they are decoded, on the CPU or copied down
/// from the GPU: for HDR10+ made for frames of a file not measured ahead.
pub(crate) struct Meter(crate::export::normalize::Relayout);

impl Meter {
    pub(crate) fn new() -> Self {
        Self(crate::export::normalize::Relayout::passing(readable))
    }

    /// `f` stays as it is: a layout to convert goes through a filter, which
    /// takes the frame it is given, so it gets another reference.
    pub(crate) fn measure(&mut self, f: &frame::Video) -> Result<FrameLight> {
        let f = if crate::hw::on_gpu(f) {
            crate::hw::download(f)?
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
        let measured = relayout.run(&prores).unwrap();
        assert_eq!(measured.format(), format::Pixel::P010LE);
    }
}
