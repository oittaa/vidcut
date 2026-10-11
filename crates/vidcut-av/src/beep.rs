//! Beep track for test fixtures: a 1 kHz tone for the first 10 ms of every
//! second, silence otherwise. The tone starts at full amplitude (cosine phase), so
//! the onset of each beep is exact to the sample in lossless pipelines and
//! measurable to well under a millisecond after lossy codecs. Comparing beep
//! onsets with barcode frame indices measures A/V offset.

/// Tone frequency in Hz.
pub const FREQ_HZ: f64 = 1000.0;
/// Beep length in milliseconds.
pub const LEN_MS: u64 = 10;
/// Peak amplitude.
pub const AMPLITUDE: f32 = 0.5;

/// Returns `count` interleaved samples for `channels` channels, starting at
/// absolute sample index `start` of the beep track at `rate` Hz.
pub fn generate(rate: u32, channels: usize, start: u64, count: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(count * channels);
    for i in 0..count as u64 {
        let v = sample_at(rate, start + i);
        out.extend(std::iter::repeat_n(v, channels));
    }
    out
}

/// Value of the beep track at absolute sample index `n`.
pub fn sample_at(rate: u32, n: u64) -> f32 {
    let rate = u64::from(rate);
    let within = n % rate;
    if within * 1000 < LEN_MS * rate {
        let t = within as f64 / rate as f64;
        (AMPLITUDE as f64 * (2.0 * std::f64::consts::PI * FREQ_HZ * t).cos()) as f32
    } else {
        0.0
    }
}

/// Finds beep onsets in a mono signal: indices where the magnitude first exceeds
/// `threshold` after at least `min_gap` samples below it.
pub fn onsets(mono: &[f32], threshold: f32, min_gap: usize) -> Vec<usize> {
    let mut found = Vec::new();
    let mut quiet = min_gap; // treat the start as preceded by silence
    for (i, &s) in mono.iter().enumerate() {
        if s.abs() > threshold {
            if quiet >= min_gap {
                found.push(i);
            }
            quiet = 0;
        } else {
            quiet += 1;
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beeps_once_per_second_with_exact_onset() {
        let rate = 48_000;
        let mono = generate(rate, 1, 0, 3 * rate as usize);
        let found = onsets(&mono, 0.1, rate as usize / 10);
        assert_eq!(found, vec![0, 48_000, 96_000]);
    }

    #[test]
    fn chunked_generation_is_continuous() {
        let rate = 44_100;
        let whole = generate(rate, 2, 0, 50_000);
        let mut parts = generate(rate, 2, 0, 1_234);
        parts.extend(generate(rate, 2, 1_234, 50_000 - 1_234));
        assert_eq!(whole, parts);
    }

    #[test]
    fn onsets_found_after_offset_start() {
        let rate = 48_000;
        // Start mid-way through second 0: first onset is at the start of second 1.
        let mono = generate(rate, 1, 30_000, 70_000);
        assert_eq!(onsets(&mono, 0.1, 4_800), vec![18_000, 66_000]);
    }
}
