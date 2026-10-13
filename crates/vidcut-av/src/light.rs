//! Light measured in pictures as a PQ export shows them: each frame's
//! brightest pixel, its average and the distribution of the brightest
//! channel (maxRGB), in cd/m². For MaxCLL and MaxFALL where the sources say
//! none, and for HDR10+ made for frames without it (decision D46). What a
//! piece shows is measured once, every frame, and the result kept with the
//! rest of its file's (`cache_in`).
//!
//! Each pixel is read as the export converts it (`color::pq_light`): PQ's
//! EOTF, HLG's inverse OETF and 1.2 system gamma on a 1000-nit display, SDR
//! as BT.1886 with its white at 203 cd/m² in BT.2020. A grid of about half
//! a million pixels is read in each frame, every pixel of smaller ones.

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

    /// MaxCLL and MaxFALL of the frames from `from` to before `to`
    /// (seconds); with none there, of the frame shown then (the next, or
    /// the last); `None` without frames.
    pub fn level(&self, from: f64, to: f64) -> Option<LightLevel> {
        let mut out: Option<(f32, f32)> = None;
        for (_, l) in self
            .frames
            .iter()
            .filter(|(t, _)| *t >= from - EPS && *t < to - EPS)
        {
            let (m, a) = out.get_or_insert((0.0, 0.0));
            *m = m.max(l.max);
            *a = a.max(l.average);
        }
        if out.is_none() {
            out = self
                .frames
                .iter()
                .find(|(t, _)| *t >= from - EPS)
                .or(self.frames.last())
                .map(|(_, l)| (l.max, l.average));
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

/// What a piece shows: its pictures and their source times (seconds).
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
            source_frame,
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
        let half = 0.5 * fps.frames_to_secs(1);
        Some(Self {
            input,
            from: origin + fps.frames_to_secs(*source_frame) - half,
            to: origin + fps.frames_to_secs(source_frame + frames) - half,
        })
    }
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
    t[((c.clamp(0.0, 1.0) * (LUT - 1) as f64).round()) as usize]
}

/// The histogram bin of `nits`: 0.1 cd/m² wide up to 100 (as fine as
/// HDR10+ counts), 1 wide above, to 10000.
fn bin(nits: f32) -> usize {
    if nits < 100.0 {
        (nits.max(0.0) * 10.0).round() as usize
    } else {
        (900.0 + nits.min(10_000.0)).round() as usize
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

fn measure_with(
    f: &frame::Video,
    relayout: &mut crate::export::video::Relayout,
) -> Result<FrameLight> {
    let f = relayout.run(f)?;
    let c = crate::color::pq_light(&f);
    let t = table(c.hdr);
    let (w, h) = (f.width() as usize, f.height() as usize);
    // About half a million pixels, on the chroma grid.
    let step = (((w * h) as f64 / 500_000.0).sqrt().ceil() as usize).max(1);
    let step = if step > 1 { step + step % 2 } else { 1 };
    let fmt = f.format();
    let (bits, semi) = match fmt {
        format::Pixel::P010LE => (10, true),
        format::Pixel::NV12 => (8, true),
        format::Pixel::YUV420P | format::Pixel::YUVJ420P => (8, false),
        other => {
            return Err(Error::Invalid(format!(
                "measuring light in {other:?} pictures"
            )));
        }
    };
    let max_code = f64::from((1u32 << bits) - 1);
    let (y0, ys, cs) = if c.full_range {
        (0.0, max_code, max_code / 2.0)
    } else {
        let k = f64::from(1u32 << (bits - 8));
        (16.0 * k, 219.0 * k, 112.0 * k)
    };
    let mid = f64::from(1u32 << (bits - 1));
    let sample = |plane: usize, x: usize, y: usize| -> f64 {
        let stride = f.stride(plane);
        let data = f.data(plane);
        if bits > 8 {
            let i = y * stride + 2 * x;
            f64::from(u16::from_le_bytes([data[i], data[i + 1]]) >> 6)
        } else {
            f64::from(data[y * stride + x])
        }
    };
    let (kr, kb) = (c.kr, c.kb);
    let kg = 1.0 - kr - kb;
    let g = c.gamut;
    let mut hist = vec![0u32; bin(10_000.0) + 1];
    let (mut max, mut sum, mut n) = (0f32, 0f64, 0u64);
    let mut maxscl = [0f32; 3];
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            let yv = (sample(0, x, y) - y0) / ys;
            let (cb, cr) = if semi {
                // Interleaved Cb, Cr.
                (
                    (sample(1, 2 * (x / 2), y / 2) - mid) / (2.0 * cs),
                    (sample(1, 2 * (x / 2) + 1, y / 2) - mid) / (2.0 * cs),
                )
            } else {
                (
                    (sample(1, x / 2, y / 2) - mid) / (2.0 * cs),
                    (sample(2, x / 2, y / 2) - mid) / (2.0 * cs),
                )
            };
            let r = yv + 2.0 * (1.0 - kr) * cr;
            let b = yv + 2.0 * (1.0 - kb) * cb;
            let gr = (yv - kr * r - kb * b) / kg;
            let mut rgb = [lut(&t, r), lut(&t, gr), lut(&t, b)];
            if c.hdr.is_none() {
                let l = rgb.map(f64::from);
                for (i, row) in g.iter().enumerate() {
                    rgb[i] = ((row[0] * l[0] + row[1] * l[1] + row[2] * l[2]).max(0.0) as f32)
                        * SDR_WHITE;
                }
            }
            let m = rgb[0].max(rgb[1]).max(rgb[2]);
            for i in 0..3 {
                maxscl[i] = maxscl[i].max(rgb[i]);
            }
            max = max.max(m);
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
        max,
        average: (sum / n.max(1) as f64) as f32,
        maxscl,
        percentiles,
    })
}

/// Measures the frames of `span`, decoded on the CPU, and the first one
/// from its end on; and whether the file ended before that. `progress`
/// hears the share done.
fn measure(
    span: &Span,
    cancel: Option<&AtomicBool>,
    progress: &mut dyn FnMut(f64),
) -> Result<(Vec<(f64, FrameLight)>, bool)> {
    crate::init();
    let mut reader = VideoReader::open_input(&span.input)?;
    let tb = reader.time_base();
    let tick = f64::from(tb.numerator()) / f64::from(tb.denominator());
    if span.from.is_finite()
        && reader
            .first_pts()
            .is_some_and(|f| (f as f64 * tick) < span.from - EPS)
    {
        reader.seek(((span.from - EPS) / tick).floor() as i64)?;
    }
    let mut relayout = crate::export::video::Relayout::new();
    let mut frames = Vec::new();
    while let Some(f) = reader.next_frame()? {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(Error::Invalid(crate::export::CANCELLED.into()));
        }
        let t = f.pts().unwrap_or(0) as f64 * tick;
        frames.push((t, measure_with(&f, &mut relayout)?));
        if t >= span.to - EPS {
            return Ok((frames, false));
        }
        if span.to.is_finite() && span.to > span.from {
            progress(((t - span.from) / (span.to - span.from)).clamp(0.0, 1.0));
        }
    }
    Ok((frames, true))
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
    let text = format!("{input:?}|{size}|{mtime}|v3");
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{hash:016x}")
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
/// disk, or (with `measure_now`) made now and kept. `None` when it isn't
/// there and `measure_now` is false. `progress` hears the share of `span`
/// measured.
pub fn analysis(
    span: &Span,
    measure_now: bool,
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
    let (frames, ended) = measure(span, cancel, progress)?;
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
    cancel: Option<&AtomicBool>,
    progress: &mut dyn FnMut(f64),
) -> Result<Option<LightLevel>> {
    let Some(span) = Span::of(piece, fps) else {
        return Ok(None);
    };
    Ok(analysis(&span, measure_now, cancel, progress)?.and_then(|a| a.level(span.from, span.to)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfers_give_their_light() {
        let pq = table(Some(Hdr::Pq));
        assert!((lut(&pq, 1.0) - 10_000.0).abs() < 1.0);
        // PQ code 573 of 1023 (limited) is 203 cd/m²: (573-64)/876.
        assert!(
            (lut(&pq, (573.0 - 64.0) / 876.0) - 203.0).abs() < 2.0,
            "{}",
            lut(&pq, (573.0 - 64.0) / 876.0)
        );
        let hlg = table(Some(Hdr::Hlg));
        assert!((lut(&hlg, 1.0) - 1000.0).abs() < 1.0);
        // HLG's 75% is about 203 cd/m² on a 1000-nit display.
        assert!((lut(&hlg, 0.75) - 203.0).abs() < 8.0, "{}", lut(&hlg, 0.75));
        assert!((lut(&table(None), 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn levels_and_frames_by_time() {
        let l = |max, average| FrameLight {
            max,
            average,
            ..FrameLight::default()
        };
        let mut a = LightAnalysis::default();
        a.add(0.0, 0.5, vec![(0.0, l(100.0, 50.0)), (0.5, l(900.0, 80.0))]);
        a.add(
            0.5,
            1.0,
            vec![(0.5, l(900.5, 80.0)), (1.0, l(300.0, 300.0))],
        );
        assert_eq!(a.frames.len(), 3);
        assert_eq!(a.spans, vec![(0.0, 1.0)]);
        assert!(a.covers(0.2, 1.0) && !a.covers(0.2, 1.5));
        assert_eq!(
            a.level(0.0, 1.0),
            Some(LightLevel {
                max_cll: 901,
                max_fall: 80
            })
        );
        assert_eq!(
            a.level(1.0, 2.0),
            Some(LightLevel {
                max_cll: 300,
                max_fall: 300
            })
        );
        // No frame there: the one shown then.
        assert_eq!(a.level(2.0, 3.0).map(|l| l.max_cll), Some(300));
        assert_eq!(a.level(0.2, 0.3).map(|l| l.max_cll), Some(901));
        assert_eq!(LightAnalysis::default().level(0.0, 1.0), None);
        assert_eq!(a.at(0.7).map(|f| f.max), Some(900.5));
        assert_eq!(from_bytes(&to_bytes(&a)), Some(a));
    }
}
