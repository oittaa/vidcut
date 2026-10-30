use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use vidcut_core::model::{Hdr, Retime, Sharpness, Upscaler};

use crate::decode::{Input, Reading, VideoReader};
use crate::error::{Error, Result};
use crate::export::video::rgb_picture;
use crate::ffmpeg::frame;
use crate::super_scale::{Out, Picture};

const FULL: (u32, u32) = (1920, 1080);
const SMALL: (u32, u32) = (480, 270);
const CUT: f64 = 50.0;
const BLOCK: usize = 40;

pub struct Clip {
    pub name: String,
    pub path: PathBuf,
    pub start: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Score {
    pub de: f64,
    pub low: f64,
    pub mean: f64,
    pub light: f64,
    pub psnr: f64,
}

pub struct Row {
    pub clip: String,
    pub test: &'static str,
    pub coding: &'static str,
    pub model: &'static str,
    pub score: Score,
    pub baseline: &'static str,
    pub base: Score,
    pub frames: usize,
}

pub fn coding_name((deep, coding): (bool, Option<Hdr>)) -> &'static str {
    match (deep, coding) {
        (_, Some(Hdr::Pq)) => "PQ",
        (_, Some(Hdr::Hlg)) => "HLG",
        (true, None) => "SDR",
        (false, None) => "SDR8",
    }
}

pub fn run(
    clip: &Clip,
    count: usize,
    (upscalers, retimes): (&[Upscaler], &[Retime]),
    dir: &Path,
) -> Result<Vec<Row>> {
    let source = decoded(clip, count)?;
    let hdr = crate::color::Source::of(&source[0]).hdr;
    let mut drawn = None;
    let mut rows = Vec::new();
    let forms = [
        (true, Some(hdr.unwrap_or(Hdr::Pq))),
        (true, None),
        (false, None),
    ];
    for form in forms {
        let coding = form.1;
        let drawn_at = |drawn: &mut _, f: &frame::Video, size, kernel| {
            rgb_picture(drawn, f, size, (kernel, form))
        };
        let refs = source
            .iter()
            .map(|f| drawn_at(&mut drawn, f, FULL, 0))
            .collect::<Result<Vec<_>>>()?;
        let truth = source
            .iter()
            .map(|f| rgb_picture(&mut drawn, f, FULL, (0, (true, coding))).and_then(|p| bytes(&p)))
            .collect::<Result<Vec<_>>>()?;
        let small = source
            .iter()
            .map(|f| drawn_at(&mut drawn, f, SMALL, 0))
            .collect::<Result<Vec<_>>>()?;
        let sharp = crate::screen::kernel(Some(Sharpness::Medium));
        let classic = small
            .iter()
            .map(|f| drawn_at(&mut drawn, f, FULL, sharp).and_then(|p| bytes(&p)))
            .collect::<Result<Vec<_>>>()?;
        let inner: Vec<usize> = (2..count.saturating_sub(2)).collect();
        let base = mean(&inner, |i| score(&classic[i], &truth[i], coding));
        for &u in upscalers {
            crate::super_scale::get_ready(dir, u, false)?;
            let made = upscaled(u, &small)?;
            rows.push(Row {
                clip: clip.name.clone(),
                test: "Super Scale",
                coding: coding_name(form),
                model: crate::super_scale::name(u),
                score: mean(&inner, |i| score(&made[i], &truth[i], coding)),
                baseline: "Classic",
                base,
                frames: inner.len(),
            });
        }
        let pairs: Vec<usize> = (0..count.saturating_sub(2)).step_by(2).collect();
        let made_by = |r: Retime| {
            pairs
                .iter()
                .map(|&i| between(r, clip, &refs, i))
                .collect::<Result<Vec<_>>>()
        };
        let blend = made_by(Retime::FrameBlend)?;
        let kept: Vec<usize> = (0..pairs.len())
            .filter(|&k| score(&blend[k], &truth[pairs[k] + 1], coding).de < CUT)
            .collect();
        let base = mean(&kept, |k| score(&blend[k], &truth[pairs[k] + 1], coding));
        for &r in retimes {
            crate::interpolate::get_ready(dir, r, false)?;
            let made = made_by(r)?;
            rows.push(Row {
                clip: clip.name.clone(),
                test: "Retime",
                coding: coding_name(form),
                model: crate::interpolate::name(r),
                score: mean(&kept, |k| score(&made[k], &truth[pairs[k] + 1], coding)),
                baseline: "Frame Blend",
                base,
                frames: kept.len(),
            });
        }
    }
    Ok(rows)
}

pub fn pictures(clip: &Clip, count: usize, k: usize, dir: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    let source = decoded(clip, count)?;
    let hdr = crate::color::Source::of(&source[0]).hdr;
    let form = (true, Some(hdr.unwrap_or(Hdr::Pq)));
    let mut drawn = None;
    let truth = bytes(&rgb_picture(&mut drawn, &source[k], FULL, (0, form))?)?;
    let small = source
        .iter()
        .map(|f| rgb_picture(&mut drawn, f, SMALL, (0, form)))
        .collect::<Result<Vec<_>>>()?;
    let sharp = crate::screen::kernel(Some(Sharpness::Medium));
    let classic = bytes(&rgb_picture(&mut drawn, &small[k], FULL, (sharp, form))?)?;
    let mut out = vec![("truth".into(), truth), ("classic".into(), classic)];
    for u in [Upscaler::AiFast, Upscaler::AiBest] {
        crate::super_scale::get_ready(dir, u, false)?;
        let mut made = upscaled(u, &small)?;
        out.push((crate::super_scale::name(u).into(), made.swap_remove(k)));
    }
    Ok(out)
}

fn decoded(clip: &Clip, count: usize) -> Result<Vec<frame::Video>> {
    let input = Input::File(clip.path.clone());
    let mut reader = VideoReader::open_as(&input, None, Reading::Frames)?;
    let tb = reader.time_base();
    let tick = f64::from(tb.numerator()) / f64::from(tb.denominator());
    reader.seek((clip.start / tick) as i64)?;
    let mut frames = Vec::with_capacity(count);
    while frames.len() < count {
        match reader.next_frame()? {
            Some(f) => frames.push(f),
            None => break,
        }
    }
    if frames.len() < count {
        return Err(Error::Invalid(format!(
            "{} has fewer than {count} frames after {} s",
            clip.path.display(),
            clip.start
        )));
    }
    Ok(frames)
}

fn bytes(f: &frame::Video) -> Result<Vec<u8>> {
    crate::screen::held::rgb(f).map(sixteen)
}

fn sixteen(rgb: Vec<u8>) -> Vec<u8> {
    match rgb.len() == whole() / 2 {
        true => rgb
            .iter()
            .flat_map(|v| (u16::from(*v) * 257).to_le_bytes())
            .collect(),
        false => rgb,
    }
}

fn whole() -> usize {
    FULL.0 as usize * FULL.1 as usize * 6
}

fn read(picture: Picture) -> Result<Vec<u8>> {
    match picture {
        Picture::Held(h) => h.read(whole()).map(sixteen),
        Picture::Rgb(b) => Ok(sixteen(b)),
        Picture::Frame(_) => Err(Error::Invalid(
            "a model gave a VideoToolbox frame for RGB input".into(),
        )),
    }
}

fn put(out: &mut [Option<Picture>], outs: Vec<Out>) -> Result<()> {
    for o in outs {
        let at = usize::try_from(o.index).unwrap_or(usize::MAX);
        if let Some(slot) = out.get_mut(at) {
            *slot = Some(o.picture);
        }
    }
    Ok(())
}

fn upscaled(u: Upscaler, small: &[frame::Video]) -> Result<Vec<Vec<u8>>> {
    upscaled_pictures(u, small)?.into_iter().map(read).collect()
}

fn upscaled_pictures(u: Upscaler, small: &[frame::Video]) -> Result<Vec<Picture>> {
    let name = crate::super_scale::name(u);
    let made = crate::super_scale::made_size(u, SMALL, FULL);
    if made != FULL {
        return Err(Error::Invalid(format!(
            "{name} makes {}x{}, not {}x{}",
            made.0, made.1, FULL.0, FULL.1
        )));
    }
    let mut out: Vec<Option<Picture>> = (0..small.len()).map(|_| None).collect();
    let mut first = 0;
    while first < small.len() {
        let last = crate::super_scale::last_frame(u, first as i64)
            .map_or(small.len() - 1, |l| (l as usize).min(small.len() - 1));
        let (tx, rx) = mpsc::channel();
        let sink: crate::super_scale::Sink = Box::new(move |outs| {
            let _ = tx.send(outs);
        });
        let mut run = crate::super_scale::open(u, SMALL, made, first as i64, sink)?;
        let mut outs = Vec::new();
        for (i, f) in small.iter().enumerate().take(last + 1).skip(first) {
            outs.extend(run.push(i as i64, Some(i as i64), f)?);
        }
        outs.extend(run.finish()?);
        put(&mut out, outs)?;
        while out[first..=last].iter().any(Option::is_none) {
            let outs = rx
                .recv_timeout(Duration::from_secs(600))
                .map_err(|_| Error::Invalid(format!("{name} left frames out")))?;
            put(&mut out, outs?)?;
        }
        drop(run);
        first = last + 1;
    }
    out.into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| Error::Invalid(format!("{name} left frames out")))
}

fn between(r: Retime, clip: &Clip, refs: &[frame::Video], i: usize) -> Result<Vec<u8>> {
    read(between_picture(r, clip, refs, i)?)
}

fn between_picture(r: Retime, clip: &Clip, refs: &[frame::Video], i: usize) -> Result<Picture> {
    let key = (clip.path.clone(), i as u64, i as i64, i as i64 + 2);
    crate::interpolate::between(r, &key, &refs[i], &refs[i + 2], FULL, 0.5)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Model {
    Upscaler(Upscaler),
    Retime(Retime),
}

pub struct Speed {
    pub per_frame: Duration,
    pub frames: usize,
    pub resident: Option<u64>,
    pub peak: Option<u64>,
}

fn allocated() -> Option<u64> {
    let gpu = &crate::screen::offscreen().ok()?.gpu;
    gpu.device
        .generate_allocator_report()
        .map(|r| r.total_allocated_bytes)
}

fn idle() -> Result<()> {
    let gpu = &crate::screen::offscreen().map_err(Error::Invalid)?.gpu;
    gpu.device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .map(drop)
        .map_err(|e| Error::Invalid(format!("the GPU did not finish: {e}")))
}

pub fn speed(
    clip: &Clip,
    count: usize,
    (model, form): (Model, crate::export::video::Form),
    dir: &Path,
) -> Result<Speed> {
    let source = decoded(clip, count)?;
    let mut drawn = None;
    let size = match model {
        Model::Upscaler(_) => SMALL,
        Model::Retime(_) => FULL,
    };
    let inputs = source
        .iter()
        .map(|f| rgb_picture(&mut drawn, f, size, (0, form)))
        .collect::<Result<Vec<_>>>()?;
    let pairs: Vec<usize> = (0..count.saturating_sub(2)).step_by(2).collect();
    let once = || -> Result<(Vec<Picture>, usize)> {
        match model {
            Model::Upscaler(u) => {
                crate::super_scale::get_ready(dir, u, false)?;
                Ok((upscaled_pictures(u, &inputs)?, inputs.len()))
            }
            Model::Retime(r) => {
                crate::interpolate::get_ready(dir, r, false)?;
                let made = pairs
                    .iter()
                    .map(|&i| between_picture(r, clip, &inputs, i))
                    .collect::<Result<Vec<_>>>()?;
                Ok((made, pairs.len()))
            }
        }
    };
    idle()?;
    let start = allocated();
    drop(once()?);
    idle()?;
    let resident = allocated();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watch = {
        let done = done.clone();
        std::thread::spawn(move || {
            let mut peak = allocated();
            while !done.load(std::sync::atomic::Ordering::Relaxed) {
                peak = peak.max(allocated());
                std::thread::sleep(Duration::from_millis(2));
            }
            peak.max(allocated())
        })
    };
    let clock = std::time::Instant::now();
    let (made, frames) = once()?;
    idle()?;
    let spent = clock.elapsed();
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    let peak = watch.join().unwrap_or(None);
    drop(made);
    Ok(Speed {
        per_frame: spent / frames.max(1) as u32,
        frames,
        resident: start.zip(resident).map(|(s, r)| r.saturating_sub(s)),
        peak: start.zip(peak).map(|(s, p)| p.saturating_sub(s)),
    })
}

fn mean(at: &[usize], score: impl Fn(usize) -> Score) -> Score {
    let n = at.len().max(1) as f64;
    let scores: Vec<Score> = at.iter().map(|&i| score(i)).collect();
    let of = |f: fn(&Score) -> f64| scores.iter().map(f).sum::<f64>() / n;
    Score {
        de: of(|s| s.de),
        low: of(|s| s.low),
        mean: of(|s| s.mean),
        light: of(|s| s.light),
        psnr: of(|s| s.psnr),
    }
}

fn sample(bytes: &[u8], p: usize) -> [f64; 3] {
    [0, 1, 2].map(|c| {
        let at = (p * 3 + c) * 2;
        f64::from(u16::from_le_bytes([bytes[at], bytes[at + 1]])) / 65535.0
    })
}

fn score(a: &[u8], b: &[u8], coding: Option<Hdr>) -> Score {
    let n = a.len().min(b.len()) / 6;
    let wide = FULL.0 as usize;
    let across = wide / BLOCK;
    let mut blocks = vec![[[0.0; 3]; 2]; across * (FULL.1 as usize / BLOCK)];
    let (mut de, mut squared) = (0.0, 0.0);
    for p in 0..n {
        let (x, y) = (sample(a, p), sample(b, p));
        squared += (0..3).map(|c| (x[c] - y[c]).powi(2)).sum::<f64>();
        let (x, y) = (nits(x, coding), nits(y, coding));
        de += apart(x, y);
        let (bx, by) = ((p % wide) / BLOCK, (p / wide) / BLOCK);
        if let Some(sum) = blocks.get_mut(by * across + bx).filter(|_| bx < across) {
            (0..3).for_each(|c| {
                sum[0][c] += x[c];
                sum[1][c] += y[c];
            });
        }
    }
    let mse = squared / (3 * n.max(1)) as f64;
    let area = (BLOCK * BLOCK) as f64;
    let count = blocks.len().max(1) as f64;
    let whole = blocks.iter().fold([[0.0; 3]; 2], |mut t, s| {
        (0..2).for_each(|i| (0..3).for_each(|c| t[i][c] += s[i][c] / area / count));
        t
    });
    let luma = |c: [f64; 3]| 0.2627 * c[0] + 0.6780 * c[1] + 0.0593 * c[2];
    Score {
        de: de / n.max(1) as f64,
        low: blocks
            .iter()
            .map(|s| apart(s[0].map(|v| v / area), s[1].map(|v| v / area)))
            .sum::<f64>()
            / count,
        mean: apart(whole[0], whole[1]),
        light: luma(whole[0]) / luma(whole[1]).max(1e-12),
        psnr: 10.0 * (1.0 / mse.max(1e-12)).log10(),
    }
}

const M1: f64 = 0.1593017578125;
const M2: f64 = 78.84375;
const C1: f64 = 0.8359375;
const C2: f64 = 18.8515625;
const C3: f64 = 18.6875;

pub fn pq_nits(code: f64) -> f64 {
    let e = code.clamp(0.0, 1.0).powf(1.0 / M2);
    10000.0 * ((e - C1).max(0.0) / (C2 - C3 * e)).powf(1.0 / M1)
}

fn pq_code(nits: f64) -> f64 {
    let y = (nits / 10000.0).clamp(0.0, 1.0).powf(M1);
    ((C1 + C2 * y) / (1.0 + C3 * y)).powf(M2)
}

fn nits(c: [f64; 3], coding: Option<Hdr>) -> [f64; 3] {
    match coding {
        Some(Hdr::Pq) => c.map(pq_nits),
        Some(Hdr::Hlg) => {
            let (a, b, k) = (0.17883277, 0.28466892, 0.55991073);
            let s = c.map(|e| {
                let e = e.clamp(0.0, 1.0);
                if e <= 0.5 {
                    e * e / 3.0
                } else {
                    (((e - k) / a).exp() + b) / 12.0
                }
            });
            let ys = 0.2627 * s[0] + 0.6780 * s[1] + 0.0593 * s[2];
            s.map(|v| 1000.0 * v * ys.max(0.0).powf(0.2))
        }
        None => {
            let l = c.map(|v| 100.0 * v.clamp(0.0, 1.0).powf(2.4));
            [
                0.6274 * l[0] + 0.3293 * l[1] + 0.0433 * l[2],
                0.0691 * l[0] + 0.9195 * l[1] + 0.0114 * l[2],
                0.0164 * l[0] + 0.0880 * l[1] + 0.8956 * l[2],
            ]
        }
    }
}

fn itp(n: [f64; 3]) -> [f64; 3] {
    let lms = [
        (1688.0 * n[0] + 2146.0 * n[1] + 262.0 * n[2]) / 4096.0,
        (683.0 * n[0] + 2951.0 * n[1] + 462.0 * n[2]) / 4096.0,
        (99.0 * n[0] + 309.0 * n[1] + 3688.0 * n[2]) / 4096.0,
    ]
    .map(pq_code);
    let [l, m, s] = lms;
    [
        0.5 * l + 0.5 * m,
        0.5 * (6610.0 * l - 13613.0 * m + 7003.0 * s) / 4096.0,
        (17933.0 * l - 17390.0 * m - 543.0 * s) / 4096.0,
    ]
}

fn apart(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (x, y) = (itp(a), itp(b));
    720.0 * (0..3).map(|i| (x[i] - y[i]).powi(2)).sum::<f64>().sqrt()
}

pub fn delta_itp(a: [f64; 3], b: [f64; 3], coding: Option<Hdr>) -> f64 {
    apart(nits(a, coding), nits(b, coding))
}

#[cfg(test)]
mod tests {
    use super::{delta_itp, pq_nits, score};
    use vidcut_core::model::Hdr;

    #[test]
    fn half_of_pq_is_92_nits() {
        assert!((pq_nits(0.5) - 92.245_708_994).abs() < 1e-6);
    }

    #[test]
    fn a_color_is_no_distance_from_itself() {
        let c = [0.4, 0.5, 0.6];
        assert_eq!(delta_itp(c, c, Some(Hdr::Pq)), 0.0);
    }

    #[test]
    fn pixels_swapped_in_a_block_keep_the_means() {
        let (white, black) = ([255u8; 6], [0u8; 6]);
        let a = [white, black].concat();
        let b = [black, white].concat();
        let s = score(&a, &b, None);
        assert_eq!((s.low, s.mean), (0.0, 0.0));
    }
}
