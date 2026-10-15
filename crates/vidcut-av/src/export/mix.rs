//! Pictures mixed while a dissolve plays (decision D50): one over another
//! at a weight, sample by sample, in the output's own samples. A sample
//! is `(under × (WHOLE − weight) + over × weight + WHOLE / 2) / WHOLE`, on
//! the CPU here and on NVIDIA's GPU (`cuda::mix`) alike, so both make the
//! same pictures.

use vidcut_core::render::WHOLE;

use crate::error::{Error, Result};
use crate::ffmpeg::{ffi, frame};

/// Sample `over` at `weight` over sample `under`.
pub(crate) fn mixed(under: u32, over: u32, weight: u32) -> u32 {
    (under * (WHOLE - weight) + over * weight + WHOLE / 2) >> 16
}

/// How a picture format's samples are stored: bytes per row of each plane
/// at a width, rows of each plane at a height, bytes per sample, and how
/// far up in them the value is (P010's six bits).
pub(crate) struct Samples {
    format: ffi::AVPixelFormat,
    pub(crate) bytes: usize,
    pub(crate) shift: u32,
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
            planes: planes.max(0) as usize,
        })
    }

    /// Bytes per row of each plane of a `width` wide picture.
    pub(crate) fn row_bytes(&self, width: u32) -> [usize; 4] {
        let mut lines = [0i32; 4];
        // SAFETY: fills four line sizes for a known format.
        unsafe { ffi::av_image_fill_linesizes(lines.as_mut_ptr(), self.format, width as i32) };
        lines.map(|l| l.max(0) as usize)
    }
}

/// `over` at `weight` over `under`, two memory pictures of the same size
/// and format, into a picture of their own.
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
    // SAFETY: both are valid frames; the properties (colors, aspect) are
    // those of the picture under.
    unsafe { ffi::av_frame_copy_props(out.as_mut_ptr(), under.as_ptr()) };
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{mix, mixed};
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
        // 64 + (940 − 64) / 4 = 283, in the high bits.
        assert_eq!(
            u16::from_le_bytes([out.data(0)[0], out.data(0)[1]]),
            283 << 6
        );
    }
}
