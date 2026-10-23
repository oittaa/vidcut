use vidcut_core::render::{WHOLE, Wipe, wipe_shows_incoming};

use crate::error::{Error, Result};
use crate::ffmpeg::{ffi, frame};

pub(crate) fn mixed(under: u32, over: u32, weight: u32) -> u32 {
    (under * (WHOLE - weight) + over * weight + WHOLE / 2) >> 16
}

pub(crate) struct Samples {
    format: ffi::AVPixelFormat,
    pub(crate) bytes: usize,
    pub(crate) shift: u32,
    pub(crate) top: u32,
    planes: usize,
}

impl Samples {
    pub(crate) fn of(format: crate::ffmpeg::format::Pixel) -> Result<Self> {
        let unknown = || Error::Invalid(format!("pictures in {format:?} can't be mixed"));
        let desc = format.descriptor().ok_or_else(unknown)?;
        // SAFETY: FFmpeg's static descriptor of the format.
        let (depth, shift, hw) = unsafe {
            let d = &*desc.as_ptr();
            (
                u32::from(d.comp[0].depth as u8),
                u32::from(d.comp[0].shift as u8),
                d.flags & (ffi::AV_PIX_FMT_FLAG_HWACCEL as u64) != 0,
            )
        };
        if hw || depth == 0 || depth + shift > 16 {
            return Err(unknown());
        }
        // SAFETY: a lookup on the format's number.
        let planes = unsafe { ffi::av_pix_fmt_count_planes(format.into()) };
        Ok(Self {
            format: format.into(),
            bytes: if depth + shift > 8 { 2 } else { 1 },
            shift,
            top: (1 << depth) - 1,
            planes: planes.max(0) as usize,
        })
    }

    pub(crate) fn row_bytes(&self, width: u32) -> [usize; 4] {
        let mut lines = [0i32; 4];
        // SAFETY: fills four line sizes for a known format.
        unsafe { ffi::av_image_fill_linesizes(lines.as_mut_ptr(), self.format, width as i32) };
        lines.map(|l| l.max(0) as usize)
    }
}

pub(crate) fn wipe(under: &frame::Video, over: &frame::Video, mask: Wipe) -> Result<frame::Video> {
    let format = under.format();
    if over.format() != format || (over.width(), over.height()) != (under.width(), under.height()) {
        return Err(Error::Invalid(
            "pictures of different sizes or formats can't be mixed".into(),
        ));
    }
    let samples = Samples::of(format)?;
    let (fw, fh) = (under.width(), under.height());
    let mut out = frame::Video::new(format, fw, fh);
    let rows = samples.row_bytes(fw);
    for (p, &bytes) in rows.iter().enumerate().take(samples.planes) {
        let height = under.plane_height(p);
        let (stride_a, stride_b, stride_o) = (under.stride(p), over.stride(p), out.stride(p));
        let (a, b) = (under.data(p), over.data(p));
        let o = out.data_mut(p);
        let sample_bytes = samples.bytes;
        let row_samples = bytes / sample_bytes;
        let (group, plane_w) = plane_group(p, fw, fh, row_samples as u32, height);
        let group = group as usize;
        for y in 0..height {
            let (ya, yb, yo) = (y as usize, y as usize, y as usize);
            let (ra, rb) = (
                &a[ya * stride_a..ya * stride_a + bytes],
                &b[yb * stride_b..yb * stride_b + bytes],
            );
            let ro = &mut o[yo * stride_o..yo * stride_o + bytes];
            let pixels = row_samples / group;
            for x in 0..pixels {
                let incoming =
                    wipe_shows_incoming(mask.direction, mask.along, x as u32, y, plane_w, height);
                let take_over = if mask.falling { !incoming } else { incoming };
                let from = if take_over { rb } else { ra };
                let at = x * group * sample_bytes;
                let n = group * sample_bytes;
                ro[at..at + n].copy_from_slice(&from[at..at + n]);
            }
        }
    }
    // SAFETY: `out` and `under` are valid frames.
    unsafe { ffi::av_frame_copy_props(out.as_mut_ptr(), under.as_ptr()) };
    Ok(out)
}

fn plane_group(
    plane: usize,
    frame_w: u32,
    frame_h: u32,
    row_samples: u32,
    plane_h: u32,
) -> (u32, u32) {
    if plane == 0 || frame_w == 0 || plane_h == 0 || row_samples == 0 {
        return (1, row_samples.max(1));
    }
    let sub_y = (frame_h / plane_h).max(1);
    if row_samples == frame_w && sub_y > 1 {
        (2, row_samples / 2)
    } else {
        (1, row_samples)
    }
}

pub(crate) fn mix(under: &frame::Video, over: &frame::Video, weight: u32) -> Result<frame::Video> {
    let format = under.format();
    if over.format() != format || (over.width(), over.height()) != (under.width(), under.height()) {
        return Err(Error::Invalid(
            "pictures of different sizes or formats can't be mixed".into(),
        ));
    }
    let samples = Samples::of(format)?;
    let mut out = frame::Video::new(format, under.width(), under.height());
    let rows = samples.row_bytes(under.width());
    for (p, &bytes) in rows.iter().enumerate().take(samples.planes) {
        let height = under.plane_height(p) as usize;
        let (sa, sb, so) = (under.stride(p), over.stride(p), out.stride(p));
        let (a, b) = (under.data(p), over.data(p));
        let o = out.data_mut(p);
        for y in 0..height {
            let (ra, rb) = (&a[y * sa..y * sa + bytes], &b[y * sb..y * sb + bytes]);
            let ro = &mut o[y * so..y * so + bytes];
            if samples.bytes == 1 {
                for ((o, &a), &b) in ro.iter_mut().zip(ra).zip(rb) {
                    *o = mixed(u32::from(a), u32::from(b), weight) as u8;
                }
            } else {
                let s = samples.shift;
                let ra = ra.as_chunks::<2>().0;
                let rb = rb.as_chunks::<2>().0;
                for ((o, a), b) in ro.as_chunks_mut::<2>().0.iter_mut().zip(ra).zip(rb) {
                    let (a, b) = (u16::from_le_bytes(*a), u16::from_le_bytes(*b));
                    let v = mixed(u32::from(a >> s), u32::from(b >> s), weight) << s;
                    *o = (v as u16).to_le_bytes();
                }
            }
        }
    }
    // SAFETY: `out` and `under` are valid frames.
    unsafe { ffi::av_frame_copy_props(out.as_mut_ptr(), under.as_ptr()) };
    Ok(out)
}

pub(crate) fn crossed(
    now: &frame::Video,
    then: &frame::Video,
    below: &frame::Video,
) -> Result<frame::Video> {
    let format = now.format();
    let size = (now.width(), now.height());
    if [then, below]
        .iter()
        .any(|f| f.format() != format || (f.width(), f.height()) != size)
    {
        return Err(Error::Invalid(
            "pictures of different sizes or formats can't be mixed".into(),
        ));
    }
    let samples = Samples::of(format)?;
    let mut out = frame::Video::new(format, size.0, size.1);
    let rows = samples.row_bytes(size.0);
    let top = i64::from(samples.top);
    for (p, &bytes) in rows.iter().enumerate().take(samples.planes) {
        let height = now.plane_height(p) as usize;
        let strides = [
            now.stride(p),
            then.stride(p),
            below.stride(p),
            out.stride(p),
        ];
        let (a, b, c) = (now.data(p), then.data(p), below.data(p));
        let o = out.data_mut(p);
        for y in 0..height {
            let (ra, rb, rc) = (
                &a[y * strides[0]..y * strides[0] + bytes],
                &b[y * strides[1]..y * strides[1] + bytes],
                &c[y * strides[2]..y * strides[2] + bytes],
            );
            let ro = &mut o[y * strides[3]..y * strides[3] + bytes];
            let sum = |a: i64, b: i64, c: i64| (a + b - c).clamp(0, top);
            if samples.bytes == 1 {
                for (i, o) in ro.iter_mut().enumerate() {
                    let v = sum(i64::from(ra[i]), i64::from(rb[i]), i64::from(rc[i]));
                    *o = v as u8;
                }
            } else {
                let s = samples.shift;
                let get = |r: &[u8], i: usize| {
                    i64::from(u16::from_le_bytes([r[2 * i], r[2 * i + 1]]) >> s)
                };
                for (i, o) in ro.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                    let v = sum(get(ra, i), get(rb, i), get(rc, i)) << s;
                    *o = (v as u16).to_le_bytes();
                }
            }
        }
    }
    // SAFETY: `out` and `now` are valid frames.
    unsafe { ffi::av_frame_copy_props(out.as_mut_ptr(), now.as_ptr()) };
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{mix, mixed, wipe};
    use crate::export::video::fill_black;
    use crate::ffmpeg::{format::Pixel, frame};

    #[test]
    fn half_way_rounds_half_up() {
        assert_eq!(mixed(16, 235, 1 << 15), 126);
    }

    #[test]
    fn a_whole_weight_shows_the_picture_over() {
        assert_eq!(mixed(16, 235, 1 << 16), 235);
    }

    #[test]
    fn a_quarter_of_white_over_black_in_p010_keeps_the_low_bits_clear() {
        let mut black = frame::Video::new(Pixel::P010LE, 16, 16);
        fill_black(&mut black).unwrap();
        let mut white = frame::Video::new(Pixel::P010LE, 16, 16);
        for px in white.data_mut(0).as_chunks_mut::<2>().0 {
            *px = (940u16 << 6).to_le_bytes();
        }
        let out = mix(&black, &white, 1 << 14).unwrap();
        assert_eq!(
            u16::from_le_bytes([out.data(0)[0], out.data(0)[1]]),
            283 << 6
        );
    }

    #[test]
    fn a_left_wipe_at_its_middle_keeps_the_left_half_of_the_picture_over() {
        use vidcut_core::model::WipeDirection;
        use vidcut_core::render::Wipe;

        let mut under = frame::Video::new(Pixel::YUV420P, 4, 2);
        fill_black(&mut under).unwrap();
        let mut over = frame::Video::new(Pixel::YUV420P, 4, 2);
        fill_black(&mut over).unwrap();
        over.data_mut(0)[..4].fill(200);
        let out = wipe(
            &under,
            &over,
            Wipe {
                direction: WipeDirection::Left,
                along: (1, 2),
                falling: false,
            },
        )
        .unwrap();
        assert_eq!(&out.data(0)[..4], &[200, 200, 16, 16]);
    }
}
