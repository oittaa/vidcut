use std::f32::consts::TAU;
use std::f64::consts::PI;

use rustfft::FftPlanner;
use rustfft::num_complex::Complex32;
use vidcut_core::place::GrainTile;

const CUBIC: f32 = 0.01245;

const FADE: f64 = 5.0;

const FLAT: f64 = 0.75;

pub(super) const BYTES: usize = 16;

const THIRD: (usize, usize) = (17, 29);

const SHIFTS: u64 = 0xd134_2543_de82_ef95;

pub(super) struct Shrunk {
    pub(super) shrink: [f64; 2],
    pub(super) soft: f64,
}

struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let z = self.0;
        let z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        let z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn normals(&mut self) -> Complex32 {
        let radius = (-2.0 * (1.0 - self.unit()).ln()).sqrt();
        let (sin, cos) = (TAU * self.unit()).sin_cos();
        Complex32::new(radius * cos, radius * sin)
    }
}

fn wrapped_frequency(i: usize, n: usize) -> f64 {
    let (i, n) = (i as f64, n as f64);
    if 2.0 * i < n { i / n } else { (i - n) / n }
}

fn folds(t: &GrainTile, reach: f64) -> (Vec<(i32, i32)>, [i32; 2]) {
    let past = |j: i32| (f64::from(j.abs()) - 0.5).max(0.0);
    let most = |stretch: f64| (reach / stretch + 0.5).ceil() as i32;
    let (nx, ny) = (most(t.aspect), most(1.0));
    let folds = (-ny..=ny)
        .flat_map(|jy| (-nx..=nx).map(move |jx| (jx, jy)))
        .filter(|&(jx, jy)| (t.aspect * past(jx)).hypot(past(jy)) < reach)
        .collect();
    (folds, [nx, ny])
}

fn kept(f: f64, shrink: f64) -> f64 {
    let r = f - f.round();
    if r == 0.0 {
        return 1.0;
    }
    let k = 1.0 / shrink;
    let d = (PI * k * r).sin() / (k * (PI * r).sin());
    d * d
}

fn softened(f: f64, sigma: f64) -> f64 {
    let taps = (super::blur::REACH * sigma).ceil() as i32;
    let (mut sum, mut total) = (1.0, 1.0);
    for d in 1..=taps {
        let d = f64::from(d);
        let w = (-d * d / (2.0 * sigma * sigma)).exp();
        sum += 2.0 * w * (2.0 * PI * f * d).cos();
        total += 2.0 * w;
    }
    sum / total
}

fn passed(g: f64, (shrink, soft): (f64, f64)) -> f64 {
    let s = softened(g, soft);
    kept(g, shrink) * s * s
}

fn kept_along(n: usize, most: i32, how: Option<(f64, f64)>) -> Vec<f64> {
    let span = (2 * most + 1) as usize;
    let mut out = vec![1.0; n * span];
    if let Some(how) = how {
        for i in 0..n {
            let f = wrapped_frequency(i, n);
            for j in -most..=most {
                out[i * span + (j + most) as usize] = passed((f + f64::from(j)) * how.0, how);
            }
        }
    }
    out
}

fn flat_along(n: usize, how: Option<(f64, f64)>) -> (Vec<f64>, f64) {
    let (shrink, _) = how.unwrap_or((1.0, 0.0));
    let most = (0.5 / shrink).ceil() as i32 + 1;
    let (mut kept_sums, mut count) = (Vec::with_capacity(n), 0.0);
    for i in 0..n {
        let f = wrapped_frequency(i, n);
        let inside = (-most..=most)
            .map(|j| (f + f64::from(j)) * shrink)
            .filter(|g| (-0.5..0.5).contains(g));
        let kept_at = |g: f64| how.map_or(1.0, |how| passed(g, how));
        let (sum, many) = inside.fold((0.0, 0.0), |(s, c), g| (s + kept_at(g), c + 1.0));
        kept_sums.push(sum);
        count += many;
    }
    (kept_sums, count / n.max(1) as f64)
}

fn amplitudes(t: &GrainTile, (w, h): (usize, usize), shrunk: &Shrunk) -> (Vec<f32>, f32) {
    let [sx, sy] = shrunk.shrink;
    let how = |s: f64| (sx < 1.0 || sy < 1.0).then_some((s, shrunk.soft / sy));
    let (hx, hy) = (how(sx), how(sy));
    let (power, kept_power) = if t.width >= FLAT * t.scale / sy * t.aspect.max(1.0) {
        let ((across, cx), (down, cy)) = (flat_along(w, hx), flat_along(h, hy));
        let power = vec![cx * cy; w * h];
        let kept_power = down
            .iter()
            .flat_map(|d| across.iter().map(move |a| a * d))
            .collect();
        (power, kept_power)
    } else {
        let two = 2.0 * t.width * t.width;
        let bump = |d: f64| (-(d * d) / two).exp();
        let reach = (t.ring + FADE * t.width) / t.scale;
        let (folds, [nx, ny]) = folds(t, reach);
        let (across, down) = (kept_along(w, nx, hx), kept_along(h, ny, hy));
        let (spanx, spany) = ((2 * nx + 1) as usize, (2 * ny + 1) as usize);
        let mut power = Vec::with_capacity(w * h);
        let mut kept_power = Vec::with_capacity(w * h);
        for y in 0..h {
            let fy = wrapped_frequency(y, h);
            for x in 0..w {
                let fx = wrapped_frequency(x, w);
                let (mut p, mut q) = (-0.0, -0.0);
                for &(jx, jy) in &folds {
                    let (gx, gy) = (f64::from(jx), f64::from(jy));
                    let d = (t.aspect * (fx + gx)).powi(2) + (fy + gy).powi(2);
                    if d < reach * reach {
                        let f = t.scale * d.sqrt();
                        let b = bump(f - t.ring) + bump(f + t.ring);
                        let a = across[x * spanx + (jx + nx) as usize];
                        p += b;
                        q += a * down[y * spany + (jy + ny) as usize] * b;
                    }
                }
                power.push(p);
                kept_power.push(q);
            }
        }
        (power, kept_power)
    };
    let mean = power.iter().sum::<f64>() / power.len().max(1) as f64;
    let kept_mean = kept_power.iter().sum::<f64>() / kept_power.len().max(1) as f64;
    let amplitudes = kept_power
        .into_iter()
        .map(|p| (p / kept_mean.max(f64::MIN_POSITIVE)).sqrt() as f32)
        .collect();
    let gain = (kept_mean / mean.max(f64::MIN_POSITIVE)).sqrt() as f32;
    (amplitudes, gain)
}

fn field(t: &GrainTile, (w, h): (usize, usize), shrunk: &Shrunk) -> (Vec<Complex32>, f32) {
    let mut random = Seeded(t.seed);
    let (amplitudes, gain) = amplitudes(t, (w, h), shrunk);
    let mut out: Vec<Complex32> = amplitudes
        .into_iter()
        .map(|a| random.normals() * a)
        .collect();
    let mut planner = FftPlanner::<f32>::new();
    let across = planner.plan_fft_inverse(w);
    for row in out.chunks_exact_mut(w) {
        across.process(row);
    }
    let down = planner.plan_fft_inverse(h);
    let mut column = vec![Complex32::new(0.0, 0.0); h];
    for x in 0..w {
        for (y, c) in column.iter_mut().enumerate() {
            *c = out[y * w + x];
        }
        down.process(&mut column);
        for (y, c) in column.iter().enumerate() {
            out[y * w + x] = *c;
        }
    }
    let scale = 1.0 / ((w * h) as f32).sqrt();
    out.iter_mut().for_each(|c| *c *= scale);
    (out, gain)
}

pub(super) fn tile_bytes(t: &GrainTile, (w, h): (u32, u32), shrunk: &Shrunk) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let (made, gain) = field(t, (w, h), shrunk);
    let spread = (1.0 + 6.0 * CUBIC + 15.0 * CUBIC * CUBIC).sqrt();
    let shaped = |u: f32| ((u + CUBIC * u * u * u) / spread * gain).to_le_bytes();
    let one = 1.0f32.to_le_bytes();
    let mut out = Vec::with_capacity(w * h * BYTES);
    for y in 0..h {
        for x in 0..w {
            let c = made[y * w + x];
            let far = made[((y + h / 2 + THIRD.1) % h) * w + (x + w / 2 + THIRD.0) % w];
            out.extend_from_slice(&shaped(c.re));
            out.extend_from_slice(&shaped(c.im));
            out.extend_from_slice(&shaped(far.re));
            out.extend_from_slice(&one);
        }
    }
    out
}

pub(super) fn shift(seed: u64, frame: i64, (w, h): (u32, u32)) -> [i32; 2] {
    let mut random = Seeded(seed ^ (frame as u64).wrapping_mul(SHIFTS));
    random.next();
    let at = random.next();
    [
        ((at >> 32) % u64::from(w.max(1))) as i32,
        ((at & 0xffff_ffff) % u64::from(h.max(1))) as i32,
    ]
}
