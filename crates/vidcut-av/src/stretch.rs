//! Playing sound faster or slower, keeping its pitch (WSOLA), knowing where
//! each block of the output came from, so the clock follows exactly what is
//! heard.
//!
//! The input is cut into fragments of `window` frames that overlap by half
//! in the output, each shaped by a Hann window and added. Fragment *k*
//! would start at *k*·hop·speed in the input; it moves by up to `search`
//! frames to where its first half best matches what would have followed the
//! fragment before, so the overlap adds alike sound and the pitch stays.
//! Output block *k* (hop frames) is the second half of fragment *k*−1 and
//! the first half of fragment *k*: it starts at the middle of fragment
//! *k*−1 and ends at the middle of fragment *k*, and those two input
//! positions are known.

/// Output frames, and where in the input they come from.
pub(crate) struct Block {
    /// Interleaved, `hop` frames.
    pub samples: Vec<f32>,
    /// Input position (frames from the stream's start) of the block's first
    /// sample.
    pub at: f64,
    /// Input frames per output frame through the block.
    pub rate: f64,
}

pub(crate) struct Stretch {
    speed: f64,
    channels: usize,
    window: usize,
    hop: usize,
    search: usize,
    hann: Vec<f32>,
    /// Input from stream frame `base` on: interleaved, and its channels'
    /// sum, which matching compares.
    input: Vec<f32>,
    mono: Vec<f32>,
    base: i64,
    /// The next fragment's number, and where the one before it starts.
    k: i64,
    prev: Option<i64>,
    /// The fragment before's windowed second half.
    tail: Vec<f32>,
}

impl Stretch {
    /// `speed` input frames per output frame (> 0), at `rate` Hz.
    pub(crate) fn new(speed: f64, rate: u32, channels: usize) -> Self {
        // About 10 ms a hop: fragments of 21 ms, moved by up to 5 ms.
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

    /// Input frames held from `base` on.
    fn end(&self) -> i64 {
        self.base + self.mono.len() as i64
    }

    /// Where fragment `k` would start.
    fn ideal(&self, k: i64) -> i64 {
        (k as f64 * self.hop as f64 * self.speed).round() as i64
    }

    /// Takes the stream's next frames (interleaved) and gives the blocks
    /// they complete.
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

    /// The stream ended: the blocks the rest of its input makes, the last
    /// one the final fragment's second half.
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

    /// The next block, when the input it needs is there (or, `finishing`,
    /// padded with silence).
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
            // The first block is the first fragment's first half.
            None => (pos as f64, 1.0),
            Some(p) => ((p + hop) as f64, (pos - p) as f64 / hop as f64),
        };
        self.prev = Some(pos);
        self.k += 1;
        // What the next fragment can still need: its natural continuation
        // and the range it is looked for in.
        let keep = (pos + hop).min(self.ideal(self.k) - search).max(self.base);
        let drop = (keep - self.base) as usize;
        if drop > 0 {
            self.input.drain(..drop * ch);
            self.mono.drain(..drop);
            self.base = keep;
        }
        Some(Block { samples, at, rate })
    }

    /// Silence after the input, up to stream frame `to`.
    fn pad(&mut self, to: i64) {
        let more = (to - self.end()).max(0) as usize;
        self.input
            .resize(self.input.len() + more * self.channels, 0.0);
        self.mono.resize(self.mono.len() + more, 0.0);
    }

    /// The start in `lo..=hi` whose first half best matches the half that
    /// starts at `natural`: coarsely, every other frame and start, then the
    /// starts beside the best.
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

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// A 10 ms 1 kHz beep every `every` frames, mono.
    fn beeps(frames: usize, every: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| {
                let w = i % every;
                if w < 480 {
                    0.5 * (std::f32::consts::TAU * w as f32 / 48.0).cos()
                } else {
                    0.0
                }
            })
            .collect()
    }

    /// Stretched in 10 ms chunks: the output and its blocks' starts.
    fn stretch(input: &[f32], speed: f64) -> (Vec<f32>, Vec<(usize, f64, f64)>) {
        let mut s = Stretch::new(speed, RATE, 1);
        let (mut out, mut marks) = (Vec::new(), Vec::new());
        let mut take = |blocks: Vec<Block>, out: &mut Vec<f32>| {
            for b in blocks {
                marks.push((out.len(), b.at, b.rate));
                out.extend(b.samples);
            }
        };
        let chunk = ((480.0 * speed) as usize).max(1);
        for c in input.chunks(chunk) {
            take(s.push(c), &mut out);
        }
        take(s.finish(), &mut out);
        (out, marks)
    }

    /// Where output frame `o` comes from, by the blocks.
    fn source(marks: &[(usize, f64, f64)], o: usize) -> f64 {
        let i = marks.partition_point(|m| m.0 <= o) - 1;
        let (start, at, rate) = marks[i];
        at + (o - start) as f64 * rate
    }

    #[test]
    fn the_length_follows_the_speed_and_the_pitch_stays() {
        let tone: Vec<f32> = (0..RATE as usize)
            .map(|i| (std::f32::consts::TAU * i as f32 / 48.0).sin() * 0.5)
            .collect();
        for speed in [0.25, 0.5, 2.0, 4.0, 8.0] {
            let (out, _) = stretch(&tone, speed);
            let expected = RATE as f64 / speed;
            assert!(
                (out.len() as f64 - expected).abs() < 1_100.0,
                "{speed}x: {} frames for {expected}",
                out.len()
            );
            // The middle, away from the fades at the ends.
            let mid = &out[out.len() / 4..out.len() * 3 / 4];
            let crossings = mid
                .windows(2)
                .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
                .count();
            let hz = crossings as f64 / 2.0 / (mid.len() as f64 / RATE as f64);
            assert!((hz - 1000.0).abs() < 20.0, "{speed}x at {hz:.0} Hz");
            let peak = mid.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!((peak - 0.5).abs() < 0.1, "{speed}x: level {peak}");
        }
    }

    #[test]
    fn the_blocks_say_where_each_sound_came_from() {
        let every = 12_000;
        let input = beeps(RATE as usize * 3, every);
        for speed in [0.5, 2.0, 4.0] {
            let (out, marks) = stretch(&input, speed);
            let found = crate::beep::onsets(&out, 0.1, (2_400.0 / speed) as usize);
            assert_eq!(found.len(), 12, "{speed}x: every beep");
            for (k, &o) in found.iter().enumerate() {
                let err = source(&marks, o) - (k * every) as f64;
                // A block blends two fragments a hop×|speed − 1| apart in
                // the input: within half of that, and the frames a fade-in
                // takes to cross the threshold.
                let bound = 250.0 * (speed - 1.0).abs() + 170.0;
                assert!(err.abs() <= bound, "{speed}x beep {k}: {err} frames off");
            }
        }
    }
}
