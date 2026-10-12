//! The pictures a player hands out: frames as decoded, in YUV, for a
//! display to turn into RGB.

use std::sync::Arc;

use crate::color::{Colors, colors_of};
use crate::error::{Error, Result};
use crate::export::video::{Normalizer, fit};
use crate::ffmpeg::{self, format, frame};
use crate::thumbs::Image;

/// Where a picture's pixels are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Memory {
    /// Planes in memory, for the CPU to read or upload.
    Cpu,
    /// A CUDA surface (NVDEC's frames).
    Cuda,
    /// A VAAPI surface.
    Vaapi,
    /// A Vulkan image.
    Vulkan,
}

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
}

/// A decoded picture, as the export's sampling rule picks it: at its own
/// size and in its own colors (`colors`), upright after `rotation`, in a
/// YUV layout. An FFmpeg frame held by reference: nothing copies its
/// pixels on their way to the screen.
///
/// A display fits it into the viewer (`placement`), turns it, and converts
/// it to BT.709 RGB with a shader that reads it as the export does;
/// `to_rgba` is that conversion on the CPU, the export's own, for a display
/// without a GPU and for checking one. Pictures are in memory today
/// ([`Memory::Cpu`]); a picture in GPU memory is the same frame with a
/// hardware format and frames context (CUDA, VAAPI, Vulkan), which the
/// engine passes along without looking at its pixels.
pub struct Picture {
    frame: frame::Video,
    rotation: u16,
    bytes: usize,
}

impl Picture {
    pub(crate) fn new(frame: frame::Video, rotation: u16) -> Self {
        let bytes = bytes_of(&frame);
        Self {
            frame,
            rotation,
            bytes,
        }
    }

    /// Stored size, before `rotation`.
    pub fn width(&self) -> u32 {
        self.frame.width()
    }

    pub fn height(&self) -> u32 {
        self.frame.height()
    }

    pub fn memory(&self) -> Memory {
        match self.frame.format() {
            format::Pixel::CUDA => Memory::Cuda,
            format::Pixel::VAAPI => Memory::Vaapi,
            format::Pixel::VULKAN => Memory::Vulkan,
            _ => Memory::Cpu,
        }
    }

    /// The samples' layout; for a GPU surface, that of its frames context.
    pub fn layout(&self) -> Layout {
        let pixel = match self.memory() {
            Memory::Cpu => self.frame.format(),
            // SAFETY: a hardware frame holds a reference to its frames
            // context, whose data is an AVHWFramesContext.
            _ => unsafe {
                let ctx = (*self.frame.as_ptr()).hw_frames_ctx;
                if ctx.is_null() {
                    format::Pixel::NV12
                } else {
                    let frames = (*ctx).data as *const ffmpeg::ffi::AVHWFramesContext;
                    format::Pixel::from((*frames).sw_format)
                }
            },
        };
        match pixel {
            format::Pixel::NV12 => Layout::Nv12,
            format::Pixel::P010LE => Layout::P010,
            _ => Layout::Yuv420p,
        }
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

    /// The memory it takes.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// The picture as export renders it at `width`×`height`: turned
    /// upright, fitted, padded black and converted to BT.709 RGB by the
    /// export's own filters; packed RGBA rows. What `frame render` gives,
    /// and what a display's shader must match. Pictures in GPU memory have
    /// no rows here.
    pub fn to_rgba(&self, width: u32, height: u32) -> Result<Image> {
        if self.memory() != Memory::Cpu {
            return Err(Error::Invalid("the picture is in GPU memory".into()));
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

/// The bytes a frame's pixels take (an estimate for hardware frames).
fn bytes_of(frame: &frame::Video) -> usize {
    let (w, h) = (frame.width() as i32, frame.height() as i32);
    // SAFETY: a size computation on plain numbers.
    let n = unsafe {
        ffmpeg::ffi::av_image_get_buffer_size(
            ffmpeg::ffi::AVPixelFormat::from(frame.format()),
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
