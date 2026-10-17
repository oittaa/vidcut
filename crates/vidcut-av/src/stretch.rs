pub(crate) struct Block {
    pub samples: Vec<f32>,
    pub at: f64,
    pub rate: f64,
}

pub(crate) struct Stretch {
    speed: f64,
    channels: usize,
    window: usize,
    hop: usize,
    search: usize,
    hann: Vec<f32>,
    input: Vec<f32>,
    mono: Vec<f32>,
    base: i64,
    k: i64,
    prev: Option<i64>,
    tail: Vec<f32>,
}

impl Stretch {
    pub(crate) fn new(speed: f64, rate: u32, channels: usize) -> Self {
        let hop = (rate as usize / 96).max(8);
        let window = 2 * hop;
        let hann = (0..window)
            .map(|j| {
                let x = std::f64::consts::TAU * j as f64 / window as f64;
                (0.5 - 0.5 * x.cos()) as f32
            })
            .collect();
        Self {
            speed,
            channels: channels.max(1),
            window,
            hop,
            search: hop / 2,
            hann,
            input: Vec::new(),
            mono: Vec::new(),
            base: 0,
            k: 0,
            prev: None,
            tail: Vec::new(),
        }
    }

    fn end(&self) -> i64 {
        self.base + self.mono.len() as i64
    }

    fn ideal(&self, k: i64) -> i64 {
        (k as f64 * self.hop as f64 * self.speed).round() as i64
    }

    pub(crate) fn push(&mut self, input: &[f32]) -> Vec<Block> {
        let ch = self.channels;
        self.input.extend_from_slice(input);
        self.mono
            .extend(input.chunks_exact(ch).map(|f| f.iter().sum::<f32>()));
        let mut out = Vec::new();
        while let Some(block) = self.next(false) {
            out.push(block);
        }
        out
    }

    pub(crate) fn finish(&mut self) -> Vec<Block> {
        let mut out = Vec::new();
        let end = self.end();
        while self.ideal(self.k) < end {
            match self.next(true) {
                Some(block) => out.push(block),
                None => break,
            }
        }
        if let Some(p) = self.prev.take() {
            out.push(Block {
                samples: std::mem::take(&mut self.tail),
                at: (p + self.hop as i64) as f64,
                rate: 1.0,
            });
        }
        out
    }

    fn next(&mut self, finishing: bool) -> Option<Block> {
        let (hop, window, search) = (self.hop as i64, self.window as i64, self.search as i64);
        let n = self.ideal(self.k);
        let pos = match self.prev {
            None => n,
            Some(p) => {
                let natural = p + hop;
                let (lo, hi) = ((n - search).max(self.base), n + search);
                if !finishing && self.end() < (hi + window).max(natural + hop) {
                    return None;
                }
                if finishing {
                    self.pad((hi + window).max(natural + hop));
                }
                self.best(natural, lo, hi)
            }
        };
        if self.end() < pos + window {
            if !finishing {
                return None;
            }
            self.pad(pos + window);
        }
        let ch = self.channels;
        let from = (pos - self.base) as usize * ch;
        let fragment: Vec<f32> = self.input[from..from + self.window * ch]
            .iter()
            .enumerate()
            .map(|(i, s)| s * self.hann[i / ch])
            .collect();
        let half = self.hop * ch;
        let mut samples = fragment[..half].to_vec();
        if self.tail.len() == half {
            for (s, t) in samples.iter_mut().zip(&self.tail) {
                *s += t;
            }
        }
        self.tail = fragment[half..].to_vec();
        let (at, rate) = match self.prev {
            None => (pos as f64, 1.0),
            Some(p) => ((p + hop) as f64, (pos - p) as f64 / hop as f64),
        };
        self.prev = Some(pos);
        self.k += 1;
        let keep = (pos + hop).min(self.ideal(self.k) - search).max(self.base);
        let drop = (keep - self.base) as usize;
        if drop > 0 {
            self.input.drain(..drop * ch);
            self.mono.drain(..drop);
            self.base = keep;
        }
        Some(Block { samples, at, rate })
    }

    fn pad(&mut self, to: i64) {
        let more = (to - self.end()).max(0) as usize;
        self.input
            .resize(self.input.len() + more * self.channels, 0.0);
        self.mono.resize(self.mono.len() + more, 0.0);
    }

    fn best(&self, natural: i64, lo: i64, hi: i64) -> i64 {
        let at = |p: i64| &self.mono[(p - self.base) as usize..][..self.hop];
        let target = at(natural);
        let score = |p: i64, step: usize| -> f32 {
            at(p)
                .iter()
                .zip(target)
                .step_by(step)
                .map(|(a, b)| a * b)
                .sum()
        };
        let pick = |candidates: &mut dyn Iterator<Item = i64>, step| {
            candidates
                .map(|p| (p, score(p, step)))
                .fold(None, |best: Option<(i64, f32)>, (p, s)| match best {
                    Some((_, b)) if b >= s => best,
                    _ => Some((p, s)),
                })
                .map_or(lo, |(p, _)| p)
        };
        let coarse = pick(&mut (lo..=hi).step_by(2), 2);
        pick(&mut ((coarse - 1).max(lo)..=(coarse + 1).min(hi)), 1)
    }
}
