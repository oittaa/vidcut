//! The pictures a player hands out.

use std::sync::Arc;

use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, format, frame, software::scaling};
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

/// A decoded picture at the player's output size and format, fitted and
/// padded as export fits it: an FFmpeg frame held by reference, never copied
/// on its way to the screen.
///
/// The engine passes pictures along without looking at their pixels. Today
/// they are in memory ([`Memory::Cpu`]); a picture in GPU memory is the same
/// frame with a hardware format and frames context (CUDA, VAAPI, Vulkan),
/// which a display imports as a texture.
pub struct Picture {
    frame: frame::Video,
    bytes: usize,
}

impl Picture {
    pub(crate) fn new(frame: frame::Video) -> Self {
        let bytes = bytes_of(&frame);
        Self { frame, bytes }
    }

    pub fn width(&self) -> u32 {
        self.frame.width()
    }

    pub fn height(&self) -> u32 {
        self.frame.height()
    }

    pub fn format(&self) -> format::Pixel {
        self.frame.format()
    }

    pub fn memory(&self) -> Memory {
        match self.frame.format() {
            format::Pixel::CUDA => Memory::Cuda,
            format::Pixel::VAAPI => Memory::Vaapi,
            format::Pixel::VULKAN => Memory::Vulkan,
            _ => Memory::Cpu,
        }
    }

    /// The frame itself: planes to upload or read, or a hardware surface.
    pub fn frame(&self) -> &frame::Video {
        &self.frame
    }

    /// The memory it takes.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// The picture as packed RGBA rows, converted when it is in another
    /// format: for drawing with the CPU and for tests. Pictures in GPU
    /// memory have no rows here.
    pub fn to_rgba(&self) -> Result<Image> {
        if self.memory() != Memory::Cpu {
            return Err(Error::Invalid("the picture is in GPU memory".into()));
        }
        let (w, h) = (self.width(), self.height());
        let converted;
        let src = if self.format() == format::Pixel::RGBA {
            &self.frame
        } else {
            let mut scaler = scaling::Context::get(
                self.format(),
                w,
                h,
                format::Pixel::RGBA,
                w,
                h,
                scaling::Flags::POINT,
            )
            .ctx(|| format!("creating {:?} -> RGBA converter", self.format()))?;
            let mut out = frame::Video::empty();
            scaler
                .run(&self.frame, &mut out)
                .ctx(|| "converting a picture to RGBA".into())?;
            converted = out;
            &converted
        };
        let stride = src.stride(0);
        let row = w as usize * 4;
        let data = src.data(0);
        let mut rgba = Vec::with_capacity(row * h as usize);
        for y in 0..h as usize {
            rgba.extend_from_slice(&data[y * stride..y * stride + row]);
        }
        Ok(Image {
            width: w,
            height: h,
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
