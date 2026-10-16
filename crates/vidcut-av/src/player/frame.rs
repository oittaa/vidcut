//! The pictures a player hands out: frames as decoded, in YUV, for a
//! display to turn into RGB.

use std::sync::Arc;

use crate::color::{Colors, colors_of};
use crate::decode::Decoding;
use crate::error::Result;
use crate::export::normalize::{Normalizer, fit};
use crate::ffmpeg::{self, format, frame};
use crate::thumbs::Image;

/// How a picture's samples are laid out: the layouts decoders give, which a
/// display's shader reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// 8-bit 4:2:0, a plane each for Y, U and V.
    Yuv420p,
    /// 8-bit 4:2:0, Y and then U and V interleaved (hardware decoders).
    Nv12,
    /// 10 bits in the high bits of 16, 4:2:0, U and V interleaved.
    P010,
    /// 10 bits in the low bits of 16, 4:2:0, a plane each.
    Yuv420p10,
    /// 8-bit 4:2:2, a plane each.
    Yuv422p,
    /// 10 bits in the low bits of 16, 4:2:2, a plane each.
    Yuv422p10,
    /// 10 bits in the high bits of 16, 4:2:2, U and V interleaved
    /// (VideoToolbox's ProRes).
    P210,
    /// 12 bits in the low bits of 16, 4:4:4, a plane each (ProRes 4444).
    /// An alpha plane after them is left out.
    Yuv444p12,
    /// 16 bits, 4:4:4, U and V interleaved (VideoToolbox's ProRes 4444).
    P416,
}

impl Layout {
    /// The layout of pictures in `p`, a format a display takes
    /// (`normalize::displayable`). 8-bit 4:2:0 for any other.
    pub(crate) fn of(p: format::Pixel) -> Self {
        use format::Pixel as P;
        match p {
            P::NV12 => Self::Nv12,
            P::P010LE => Self::P010,
            P::YUV420P10LE => Self::Yuv420p10,
            P::YUV422P | P::YUVJ422P => Self::Yuv422p,
            P::YUV422P10LE => Self::Yuv422p10,
            P::P210LE => Self::P210,
            P::YUV444P12LE | P::YUVA444P12LE => Self::Yuv444p12,
            P::P416LE => Self::P416,
            _ => Self::Yuv420p,
        }
    }
}

/// A decoded picture, as the export's sampling rule picks it: at its own
/// size and in its own colors (`colors`), upright after `rotation`, in a
/// YUV layout. An FFmpeg frame held by reference: nothing copies its
/// pixels on their way to the screen.
///
/// A display fits it into the viewer (`placement`), turns it, and converts
/// it to BT.709 RGB with a shader that reads it as the export does;
/// `to_rgba` is that conversion on the CPU, the export's own, for a display
/// without a GPU and for checking one. A picture is in memory, or, decoded
/// on the GPU for a display that shows it from there, in the GPU's memory
/// ([`gpu`](Self::gpu)), which the engine passes along without looking at
/// its pixels.
///
/// While a dissolve plays, a frame's picture is the one under the others,
/// and those over it, each at its level ([`over`](Self::over), decision
/// D50): a display draws each over the ones before.
pub struct Picture {
    frame: frame::Video,
    rotation: u16,
    bytes: usize,
    decoder: Option<super::Decoder>,
    decoding: Option<Arc<Decoding>>,
    over: Vec<Over>,
}

/// A picture over another while a dissolve plays, at its weight
/// (`render::Ramp::weight`: [`WHOLE`](vidcut_core::render::WHOLE) shows
/// all of it).
#[derive(Clone)]
pub struct Over {
    pub picture: Arc<Picture>,
    pub weight: u32,
}

impl Over {
    /// Its level, 0 to 1.
    pub fn level(&self) -> f32 {
        (f64::from(self.weight) / f64::from(vidcut_core::render::WHOLE)) as f32
    }
}

impl Picture {
    /// `decoder`: what decoded it (`None` for black); `decoding`, what its
    /// file holds and what decoded it, for people.
    pub(crate) fn new(
        frame: frame::Video,
        rotation: u16,
        decoder: Option<super::Decoder>,
        decoding: Option<Arc<Decoding>>,
    ) -> Self {
        let bytes = bytes_of(&frame);
        Self {
            frame,
            rotation,
            bytes,
            decoder,
            decoding,
            over: Vec::new(),
        }
    }

    /// The picture with `over` on top of it, bottom up.
    pub(crate) fn with_over(mut self, over: Vec<Over>) -> Self {
        self.over = over;
        self
    }

    /// The pictures over it while a dissolve plays, bottom up; none
    /// otherwise.
    pub fn over(&self) -> &[Over] {
        &self.over
    }

    /// What its file holds and what decoded it: the codec, profile and
    /// rate, FFmpeg's decoder and where it ran, and why that was the CPU
    /// when a GPU was asked (`VideoReader::decoding`). `None` for black.
    pub fn decoding(&self) -> Option<&Decoding> {
        self.decoding.as_deref()
    }

    /// What decoded it: the CPU or the GPU; `None` for black.
    pub fn decoder(&self) -> Option<super::Decoder> {
        self.decoder
    }

    /// Stored size, before `rotation`.
    pub fn width(&self) -> u32 {
        self.frame.width()
    }

    pub fn height(&self) -> u32 {
        self.frame.height()
    }

    /// The GPU decoder in whose memory it is; `None` in memory. The
    /// pictures over it say where they are.
    pub fn gpu(&self) -> Option<crate::hw::Kind> {
        crate::hw::Kind::of_frame(&self.frame)
    }

    /// The GPU decoder in whose memory it is, or a picture over it is;
    /// `None` when all are in memory.
    pub(crate) fn in_video_memory(&self) -> Option<crate::hw::Kind> {
        self.gpu()
            .or_else(|| self.over.iter().find_map(|o| o.picture.in_video_memory()))
    }

    /// The samples' layout; for a GPU surface, that of its frames context.
    pub fn layout(&self) -> Layout {
        Layout::of(crate::hw::sw_format(&self.frame))
    }

    /// How its samples are read to get BT.709 RGB (matrix, range, HDR).
    pub fn colors(&self) -> Colors {
        colors_of(&self.frame)
    }

    /// The clockwise turn (0, 90, 180 or 270) that shows it upright.
    pub fn rotation(&self) -> u16 {
        self.rotation
    }

    /// Where it goes in a `width`×`height` picture, as export fits it:
    /// turned upright, keeping the display aspect, centered, the rest
    /// black. `(x, y, w, h)` of the upright picture.
    pub fn placement(&self, width: u32, height: u32) -> (u32, u32, u32, u32) {
        let sar = self.frame.aspect_ratio();
        let sar = (sar.numerator(), sar.denominator());
        let (w, h) = if self.rotation % 180 == 90 {
            fit(self.height(), self.width(), (sar.1, sar.0), width, height)
        } else {
            fit(self.width(), self.height(), sar, width, height)
        };
        ((width - w) / 2, (height - h) / 2, w, h)
    }

    /// The frame itself: planes to upload or read, or a hardware surface.
    pub fn frame(&self) -> &frame::Video {
        &self.frame
    }

    /// The picture with its planes in memory: copied down from GPU memory,
    /// or itself (by reference) when it is in memory already; those over
    /// it too.
    pub fn copied_down(&self) -> Result<Picture> {
        let frame = match self.gpu() {
            None => crate::export::video::frame_ref(&self.frame)?,
            Some(_) => crate::hw::download(&self.frame)?,
        };
        let over = self
            .over
            .iter()
            .map(|o| {
                Ok(Over {
                    picture: Arc::new(o.picture.copied_down()?),
                    weight: o.weight,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self::new(frame, self.rotation, self.decoder, self.decoding.clone()).with_over(over))
    }

    /// The memory it takes, with the pictures over it.
    pub fn bytes(&self) -> usize {
        self.bytes + self.over.iter().map(|o| o.picture.bytes()).sum::<usize>()
    }

    /// It and the pictures over it, each with the memory its own frame
    /// takes, and the GPU decoder in whose memory that is (`None`: in
    /// memory).
    pub(crate) fn layers(&self) -> Vec<(usize, Option<crate::hw::Kind>)> {
        std::iter::once((self.bytes, self.gpu()))
            .chain(self.over.iter().flat_map(|o| o.picture.layers()))
            .collect()
    }

    /// The picture as export renders it at `width`×`height`: turned
    /// upright, fitted, padded black and converted to BT.709 RGB by the
    /// export's own filters; packed RGBA rows. What `frame render` gives,
    /// and what a display's shader must match. Pictures in GPU memory are
    /// copied down first.
    pub fn to_rgba(&self, width: u32, height: u32) -> Result<Image> {
        if self.in_video_memory().is_some() {
            return self.copied_down()?.to_rgba(width, height);
        }
        if !self.over.is_empty() {
            let alone = Self::new(
                crate::export::video::frame_ref(&self.frame)?,
                self.rotation,
                self.decoder,
                self.decoding.clone(),
            );
            let mut mixed = alone.to_rgba(width, height)?;
            for o in &self.over {
                let top = o.picture.to_rgba(width, height)?;
                mixed = crate::export::renderer::mixed_image(&mixed, &top, o.weight);
            }
            return Ok(mixed);
        }
        let mut normalizer = Normalizer::new(
            &self.frame,
            self.rotation,
            width,
            height,
            format::Pixel::RGBA,
            None,
        )?;
        let out = normalizer.run(&self.frame)?;
        let stride = out.stride(0);
        let row = width as usize * 4;
        let data = out.data(0);
        let mut rgba = Vec::with_capacity(row * height as usize);
        for y in 0..height as usize {
            rgba.extend_from_slice(&data[y * stride..y * stride + row]);
        }
        Ok(Image {
            width,
            height,
            rgba,
        })
    }
}

/// The bytes a frame's pixels take, in memory or in GPU memory (its
/// frames' layout). A CUDA frame's buffer is all of its memory, rows padded
/// to CUDA's alignment.
fn bytes_of(frame: &frame::Video) -> usize {
    // SAFETY: reads the frame's own buffer reference.
    let held = unsafe {
        let buf = (*frame.as_ptr()).buf[0];
        if buf.is_null() { 0 } else { (*buf).size }
    };
    if frame.format() == format::Pixel::CUDA && held > 0 {
        return held;
    }
    let (w, h) = (frame.width() as i32, frame.height() as i32);
    // SAFETY: a size computation on plain numbers.
    let n = unsafe {
        ffmpeg::ffi::av_image_get_buffer_size(
            ffmpeg::ffi::AVPixelFormat::from(crate::hw::sw_format(frame)),
            w,
            h,
            1,
        )
    };
    if n > 0 {
        n as usize
    } else {
        (w.max(1) as usize * h.max(1) as usize * 3).div_ceil(2)
    }
}

/// What the viewer shows now.
#[derive(Clone)]
pub struct Shown {
    /// The timeline frame the clock is at.
    pub frame: i64,
    pub picture: Arc<Picture>,
    /// The picture is that frame's. `false`: the frame isn't decoded yet,
    /// and this is the picture shown before it.
    pub exact: bool,
}

#[cfg(test)]
mod tests {
    use super::{Layout, Picture};
    use crate::ffmpeg::{ffi, format::Pixel, frame};

    #[test]
    fn a_picture_in_cuda_memory_takes_what_its_buffer_holds() {
        let mut f = frame::Video::empty();
        f.set_format(Pixel::CUDA);
        f.set_width(960);
        f.set_height(540);
        // SAFETY: a fresh frame given a buffer of its own.
        unsafe { (*f.as_mut_ptr()).buf[0] = ffi::av_buffer_alloc(1_658_880) };
        assert_eq!(Picture::new(f, 0, None, None).bytes(), 1_658_880);
    }

    #[test]
    fn yuv420p10le_pictures_are_10_bit_planar_4_2_0() {
        assert_eq!(Layout::of(Pixel::YUV420P10LE), Layout::Yuv420p10);
    }

    #[test]
    fn yuv422p_pictures_are_8_bit_planar_4_2_2() {
        assert_eq!(Layout::of(Pixel::YUV422P), Layout::Yuv422p);
    }

    #[test]
    fn full_range_yuvj422p_pictures_are_8_bit_planar_4_2_2() {
        assert_eq!(Layout::of(Pixel::YUVJ422P), Layout::Yuv422p);
    }

    #[test]
    fn yuv422p10le_pictures_are_10_bit_planar_4_2_2() {
        assert_eq!(Layout::of(Pixel::YUV422P10LE), Layout::Yuv422p10);
    }

    #[test]
    fn p210le_pictures_are_10_bit_interleaved_4_2_2() {
        assert_eq!(Layout::of(Pixel::P210LE), Layout::P210);
    }

    #[test]
    fn yuv444p12le_pictures_are_12_bit_planar_4_4_4() {
        assert_eq!(Layout::of(Pixel::YUV444P12LE), Layout::Yuv444p12);
    }

    #[test]
    fn yuva444p12le_pictures_are_12_bit_planar_4_4_4_without_their_alpha() {
        assert_eq!(Layout::of(Pixel::YUVA444P12LE), Layout::Yuv444p12);
    }

    #[test]
    fn p416le_pictures_are_16_bit_interleaved_4_4_4() {
        assert_eq!(Layout::of(Pixel::P416LE), Layout::P416);
    }
}
