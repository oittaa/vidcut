use std::f32::consts::TAU;

use rustfft::FftPlanner;
use rustfft::num_complex::Complex32;
use vidcut_core::place::GrainTile;

const CUBIC: f32 = 0.01245;

const FADE: f64 = 5.0;

const FLAT: f64 = 0.75;

pub(super) const BYTES: usize = 16;

const THIRD: (usize, usize) = (17, 29);

const SHIFTS: u64 = 0xd134_2543_de82_ef95;

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

fn folds(t: &GrainTile, reach: f64) -> Vec<(f64, f64)> {
    let past = |j: i32| (f64::from(j.abs()) - 0.5).max(0.0);
    let most = |stretch: f64| (reach / stretch + 0.5).ceil() as i32;
    let (nx, ny) = (most(t.aspect), most(1.0));
    (-ny..=ny)
        .flat_map(|jy| (-nx..=nx).map(move |jx| (jx, jy)))
        .filter(|&(jx, jy)| (t.aspect * past(jx)).hypot(past(jy)) < reach)
        .map(|(jx, jy)| (f64::from(jx), f64::from(jy)))
        .collect()
}

fn amplitudes(t: &GrainTile, (w, h): (usize, usize)) -> Vec<f32> {
    if t.width >= FLAT * t.scale * t.aspect.max(1.0) {
        return vec![1.0; w * h];
    }
    let two = 2.0 * t.width * t.width;
    let bump = |d: f64| (-(d * d) / two).exp();
    let reach = (t.ring + FADE * t.width) / t.scale;
    let folds = folds(t, reach);
    let mut power = Vec::with_capacity(w * h);
    for y in 0..h {
        let fy = wrapped_frequency(y, h);
        for x in 0..w {
            let fx = wrapped_frequency(x, w);
            let p = folds
                .iter()
                .map(|&(jx, jy)| (t.aspect * (fx + jx)).powi(2) + (fy + jy).powi(2))
                .filter(|&d| d < reach * reach)
                .map(|d| t.scale * d.sqrt())
                .map(|f| bump(f - t.ring) + bump(f + t.ring))
                .sum::<f64>();
            power.push(p);
        }
    }
    let mean = power.iter().sum::<f64>() / power.len().max(1) as f64;
    power
        .into_iter()
        .map(|p| (p / mean.max(f64::MIN_POSITIVE)).sqrt() as f32)
        .collect()
}

fn field(t: &GrainTile, (w, h): (usize, usize)) -> Vec<Complex32> {
    let mut random = Seeded(t.seed);
    let mut out: Vec<Complex32> = amplitudes(t, (w, h))
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
    out
}

pub(super) fn tile_bytes(t: &GrainTile, (w, h): (u32, u32)) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let made = field(t, (w, h));
    let spread = (1.0 + 6.0 * CUBIC + 15.0 * CUBIC * CUBIC).sqrt();
    let shaped = |u: f32| ((u + CUBIC * u * u * u) / spread).to_le_bytes();
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
