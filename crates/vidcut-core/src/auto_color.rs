pub const AUTO_LEVELS: usize = 100;

const AUTO_STEPS: f64 = 99.0;

const AUTO_SHARE: f64 = 0.1925;

const AUTO_LIFT_LEAST: f64 = -2.0;

const AUTO_LUMA: [f64; 3] = [0.299, 0.587, 0.114];

#[derive(Debug, Clone, PartialEq)]
pub struct Levels {
    pub counts: [[u64; AUTO_LEVELS]; 3],
}

impl Levels {
    pub fn of(pixels: impl IntoIterator<Item = [f32; 3]>) -> Self {
        let mut counts = [[0; AUTO_LEVELS]; 3];
        for px in pixels {
            for (c, v) in px.into_iter().enumerate() {
                counts[c][step(v)] += 1;
            }
        }
        Self { counts }
    }
}

fn step(v: f32) -> usize {
    let v = f64::from(v);
    match v.is_nan() {
        true => 0,
        false => (AUTO_STEPS * v.clamp(0.0, 1.0)).floor() as usize,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Balance {
    pub lift: [f64; 4],
    pub gain: [f64; 4],
}

pub fn legacy(levels: &Levels) -> Balance {
    let points = levels.counts.map(|c| points(&c));
    let weighted = |pick: fn(&(f64, f64)) -> f64| -> f64 {
        points.iter().zip(AUTO_LUMA).map(|(p, w)| w * pick(p)).sum()
    };
    let master = (weighted(|p| p.0), weighted(|p| p.1));
    let all = [points[0], points[1], points[2], master];
    Balance {
        lift: all.map(|(lo, _)| (-lo / (1.0 - lo)).max(AUTO_LIFT_LEAST) / 2.0),
        gain: all.map(|(_, hi)| 1.0 / hi),
    }
}

fn points(counts: &[u64; AUTO_LEVELS]) -> (f64, f64) {
    let all: u64 = counts.iter().sum();
    if all == 0 {
        return (0.0, 1.0);
    }
    let k = ((all as f64 * AUTO_SHARE).round() as u64).clamp(1, all);
    let low = mean_of(counts.iter().copied().enumerate(), k) / AUTO_STEPS;
    let high = mean_of(counts.iter().copied().enumerate().rev(), k) / AUTO_STEPS;
    (low, high + (1.0 - low) / AUTO_STEPS)
}

fn mean_of(levels: impl Iterator<Item = (usize, u64)>, k: u64) -> f64 {
    let mut left = k;
    let mut sum = 0.0;
    for (b, n) in levels {
        let take = n.min(left);
        sum += b as f64 * take as f64;
        left -= take;
        if left == 0 {
            break;
        }
    }
    sum / k as f64
}
