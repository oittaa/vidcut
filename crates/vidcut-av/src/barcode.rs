pub const BITS: usize = 20;

const LOW: u8 = 16;
const HIGH: u8 = 235;
const MID: u8 = 128;
const MIN_CONTRAST: i32 = 40;

pub const MAX_INDEX: u32 = (1 << BITS) - 1;

pub fn paint(index: u32, luma: &mut [u8], width: usize, height: usize, stride: usize) {
    assert!(
        index <= MAX_INDEX,
        "barcode index {index} exceeds {MAX_INDEX}"
    );
    assert!(
        width >= BITS && height >= 2,
        "plane {width}x{height} too small"
    );
    assert!(luma.len() >= stride * height, "luma buffer too short");
    let cell = width / BITS;
    let half = height / 2;
    for y in 0..height {
        let row = &mut luma[y * stride..y * stride + width];
        let inverted = y >= half;
        for (x, px) in row.iter_mut().enumerate() {
            let bit = x / cell;
            *px = if bit >= BITS {
                MID
            } else {
                let one = (index >> (BITS - 1 - bit)) & 1 == 1;
                if one != inverted { HIGH } else { LOW }
            };
        }
    }
}

pub fn read(luma: &[u8], width: usize, height: usize, stride: usize) -> Option<u32> {
    read_with(width, height, |x, y| luma[y * stride + x])
}

pub fn read_packed(
    data: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    bytes_per_pixel: usize,
    offset: usize,
) -> Option<u32> {
    read_with(width, height, |x, y| {
        data[y * stride + x * bytes_per_pixel + offset]
    })
}

pub fn read_with(width: usize, height: usize, px: impl Fn(usize, usize) -> u8) -> Option<u32> {
    if width < BITS * 4 || height < 8 {
        return None;
    }
    let painted = width / BITS;
    read_cells(height, &px, |quarters| quarters * painted / 4)
        .or_else(|| read_cells(height, &px, |quarters| quarters * width / (BITS * 4)))
}

fn read_cells(
    height: usize,
    px: &impl Fn(usize, usize) -> u8,
    x: impl Fn(usize) -> usize,
) -> Option<u32> {
    let half = height / 2;
    let mean = |x0: usize, x1: usize, y0: usize, y1: usize| -> i32 {
        let (mut sum, mut n) = (0u64, 0u64);
        for y in y0..y1 {
            for x in x0..x1 {
                sum += u64::from(px(x, y));
                n += 1;
            }
        }
        (sum / n.max(1)) as i32
    };
    let (ty0, ty1) = (half / 4, half * 3 / 4);
    let (by0, by1) = (half + half / 4, half + half * 3 / 4);
    let mut index = 0u32;
    for bit in 0..BITS {
        let (x0, x1) = (x(bit * 4 + 1), x(bit * 4 + 3));
        let top = mean(x0, x1.max(x0 + 1), ty0, ty1.max(ty0 + 1));
        let bottom = mean(x0, x1.max(x0 + 1), by0, by1.max(by0 + 1));
        let mid = i32::from(MID);
        if (top - mid).abs() < MIN_CONTRAST || (bottom - mid).abs() < MIN_CONTRAST {
            return None;
        }
        let top_one = top > mid;
        if top_one == (bottom > mid) {
            return None;
        }
        index = (index << 1) | u32::from(top_one);
    }
    Some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(index: u32, width: usize, height: usize, stride: usize) -> Option<u32> {
        let mut buf = vec![0u8; stride * height];
        paint(index, &mut buf, width, height, stride);
        read(&buf, width, height, stride)
    }

    #[test]
    fn roundtrips_across_sizes_and_strides() {
        for &(w, h, s) in &[
            (320, 180, 320),
            (320, 180, 384),
            (1920, 1080, 1920),
            (100, 8, 128),
        ] {
            for index in [0, 1, 2, 29, 30, 1000, 65_535, MAX_INDEX] {
                assert_eq!(
                    roundtrip(index, w, h, s),
                    Some(index),
                    "{w}x{h}/{s} #{index}"
                );
            }
        }
    }

    #[test]
    fn rejects_blank_and_uniform_frames() {
        let gray = vec![MID; 320 * 180];
        assert_eq!(read(&gray, 320, 180, 320), None);
        let white = vec![HIGH; 320 * 180];
        assert_eq!(
            read(&white, 320, 180, 320),
            None,
            "top and bottom bands must disagree"
        );
    }

    #[test]
    fn survives_mild_noise() {
        let (w, h) = (320, 180);
        let mut buf = vec![0u8; w * h];
        paint(12_345, &mut buf, w, h, w);
        for (i, px) in buf.iter_mut().enumerate() {
            let noise = ((i * 7919) % 31) as i32 - 15;
            *px = (i32::from(*px) + noise).clamp(0, 255) as u8;
        }
        assert_eq!(read(&buf, w, h, w), Some(12_345));
    }

    #[test]
    fn reads_packed_rgb0() {
        let (w, h) = (160, 90);
        let mut luma = vec![0u8; w * h];
        paint(777, &mut luma, w, h, w);
        let stride = w * 4 + 64;
        let mut rgb0 = vec![0u8; stride * h];
        for y in 0..h {
            for x in 0..w {
                let v = luma[y * w + x];
                rgb0[y * stride + x * 4..y * stride + x * 4 + 3].copy_from_slice(&[v, v, v]);
            }
        }
        assert_eq!(read_packed(&rgb0, w, h, stride, 4, 1), Some(777));
    }
}
