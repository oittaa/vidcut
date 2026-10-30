use std::thread;

const LEVELS: u32 = 5;

pub(crate) fn fixed(
    made: &[u8],
    size: (u32, u32),
    source: &[u8],
    from: (u32, u32),
    deep: bool,
) -> Vec<u8> {
    let (w, h) = (size.0 as usize, size.1 as usize);
    let picture = values(made, deep);
    let reference = enlarged(
        &values(source, deep),
        (from.0 as usize, from.1 as usize),
        (w, h),
    );
    let mut low: Vec<f32> = reference.iter().zip(&picture).map(|(r, v)| r - v).collect();
    blurred(&mut low, (w, h));
    let out: Vec<f32> = picture
        .iter()
        .zip(&low)
        .map(|(v, d)| (v + d).clamp(0.0, 1.0))
        .collect();
    bytes(&out, deep)
}

fn values(rgb: &[u8], deep: bool) -> Vec<f32> {
    match deep {
        true => rgb
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| f32::from(u16::from_le_bytes(*c)) / 65535.0)
            .collect(),
        false => rgb.iter().map(|v| f32::from(*v) / 255.0).collect(),
    }
}

fn bytes(v: &[f32], deep: bool) -> Vec<u8> {
    match deep {
        true => v
            .iter()
            .flat_map(|x| ((x * 65535.0).round() as u16).to_le_bytes())
            .collect(),
        false => v.iter().map(|x| (x * 255.0).round() as u8).collect(),
    }
}

fn enlarged(src: &[f32], (sw, sh): (usize, usize), (w, h): (usize, usize)) -> Vec<f32> {
    let at = |o: usize, n: usize, s: usize| {
        let p = ((o as f32 + 0.5) * s as f32 / n as f32 - 0.5).clamp(0.0, (s - 1) as f32);
        let i = p.floor() as usize;
        (i, (i + 1).min(s - 1), p - i as f32)
    };
    let cols: Vec<_> = (0..w).map(|x| at(x, w, sw)).collect();
    let mut out = vec![0.0; w * h * 3];
    rows(&mut out, w * 3, |y, row| {
        let (y0, y1, fy) = at(y, h, sh);
        for (px, &(x0, x1, fx)) in row.as_chunks_mut::<3>().0.iter_mut().zip(&cols) {
            for (c, v) in px.iter_mut().enumerate() {
                let s = |yy: usize, xx: usize| src[(yy * sw + xx) * 3 + c];
                let top = s(y0, x0) + (s(y0, x1) - s(y0, x0)) * fx;
                let bottom = s(y1, x0) + (s(y1, x1) - s(y1, x0)) * fx;
                *v = top + (bottom - top) * fy;
            }
        }
    });
    out
}

fn blurred(v: &mut [f32], (w, h): (usize, usize)) {
    let stride = w * 3;
    let mut across = vec![0.0; v.len()];
    for level in 0..LEVELS {
        let d = 1usize << level;
        let line = &*v;
        rows(&mut across, stride, |y, row| {
            let src = &line[y * stride..][..stride];
            for x in 0..w {
                let (a, b) = (x.saturating_sub(d), (x + d).min(w - 1));
                for c in 0..3 {
                    row[x * 3 + c] =
                        0.25 * src[a * 3 + c] + 0.5 * src[x * 3 + c] + 0.25 * src[b * 3 + c];
                }
            }
        });
        let done = &across;
        rows(v, stride, |y, row| {
            let (a, b) = (y.saturating_sub(d), (y + d).min(h - 1));
            let (up, mid, down) = (
                &done[a * stride..][..stride],
                &done[y * stride..][..stride],
                &done[b * stride..][..stride],
            );
            for (i, out) in row.iter_mut().enumerate() {
                *out = 0.25 * up[i] + 0.5 * mid[i] + 0.25 * down[i];
            }
        });
    }
}

fn rows(out: &mut [f32], stride: usize, each: impl Fn(usize, &mut [f32]) + Sync) {
    let threads = thread::available_parallelism().map_or(1, |n| n.get());
    let per = (out.len() / stride).div_ceil(threads).max(1);
    let each = &each;
    thread::scope(|s| {
        for (k, chunk) in out.chunks_mut(per * stride).enumerate() {
            s.spawn(move || {
                for (i, row) in chunk.chunks_exact_mut(stride).enumerate() {
                    each(k * per + i, row);
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::fixed;

    #[test]
    fn a_flat_picture_takes_the_flat_color_of_its_source() {
        assert_eq!(
            fixed(&[100; 12], (2, 2), &[120, 120, 120], (1, 1), false),
            [120; 12]
        );
    }

    #[test]
    fn a_16_bit_picture_stays_16_bit() {
        assert_eq!(
            fixed(
                &[0xE8, 0x03].repeat(6),
                (2, 1),
                &[0xB8, 0x0B].repeat(3),
                (1, 1),
                true
            ),
            [0xB8, 0x0B].repeat(6)
        );
    }
}
