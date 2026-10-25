use vidcut_core::render::{WHOLE, Wipe, wipe_shows_incoming};

use super::place::Canvas;
use crate::error::{Error, Result};
use crate::ffmpeg::ffi;

pub(crate) struct Samples {
    format: ffi::AVPixelFormat,
    pub(crate) bytes: usize,
    pub(crate) shift: u32,
    pub(crate) top: u32,
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
        Ok(Self {
            format: format.into(),
            bytes: if depth + shift > 8 { 2 } else { 1 },
            shift,
            top: (1 << depth) - 1,
        })
    }

    pub(crate) fn row_bytes(&self, width: u32) -> [usize; 4] {
        let mut lines = [0i32; 4];
        // SAFETY: fills four line sizes for a known format.
        unsafe { ffi::av_image_fill_linesizes(lines.as_mut_ptr(), self.format, width as i32) };
        lines.map(|l| l.max(0) as usize)
    }
}

pub(crate) fn wipe(under: &Canvas, over: &Canvas, mask: Wipe) -> Result<Canvas> {
    under.picked(over, |[x, y, width, height]| {
        wipe_shows_incoming(mask.direction, mask.along, x, y, width, height) != mask.falling
    })
}

pub(crate) fn mix(under: &Canvas, over: &Canvas, weight: u32) -> Result<Canvas> {
    let w = weight as f32 / WHOLE as f32;
    under.zipped(over, |a, b| a + (b - a) * w)
}

pub(crate) fn crossed(now: &Canvas, then: &Canvas, below: &Canvas) -> Result<Canvas> {
    now.zipped(&then.zipped(below, |b, c| b - c)?, |a, d| a + d)
}

#[cfg(test)]
mod tests {
    use super::{mix, wipe};
    use crate::export::place::Canvas;
    use crate::export::video::fill_black;
    use crate::ffmpeg::{format::Pixel, frame};

    #[test]
    fn a_quarter_of_white_over_black_in_p010_keeps_the_low_bits_clear() {
        let mut black = frame::Video::new(Pixel::P010LE, 16, 16);
        fill_black(&mut black).unwrap();
        let mut white = frame::Video::new(Pixel::P010LE, 16, 16);
        for px in white.data_mut(0).as_chunks_mut::<2>().0 {
            *px = (940u16 << 6).to_le_bytes();
        }
        let (black, white) = (Canvas::of(&black).unwrap(), Canvas::of(&white).unwrap());
        let out = mix(&black, &white, 1 << 14).unwrap().frame();
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
            &Canvas::of(&under).unwrap(),
            &Canvas::of(&over).unwrap(),
            Wipe {
                direction: WipeDirection::Left,
                along: (1, 2),
                falling: false,
            },
        )
        .unwrap()
        .frame();
        assert_eq!(&out.data(0)[..4], &[200, 200, 16, 16]);
    }
}
