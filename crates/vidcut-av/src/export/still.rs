use std::path::Path;

use vidcut_core::render::VideoPiece;
use vidcut_core::time::Rational;

use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, codec, encoder, format};

pub fn png(
    piece: &VideoPiece,
    fps: Rational,
    ((width, height), frame): ((u32, u32), (u32, u32)),
    colors: super::renderer::Finish<'_>,
    decoder: Option<&crate::hw::GpuDecoder>,
) -> Result<Vec<u8>> {
    crate::cancel::stop()?;
    crate::init();
    let pixel = format::Pixel::RGB24;
    let mut frames =
        super::renderer::frames(fps, (width, height), pixel, colors, decoder)?.in_frame(frame);
    frames.read_piece(piece);
    let frame = frames
        .next_frame()?
        .ok_or_else(|| Error::Invalid("no frame to render".into()))?;
    encoded(frame)
}

pub fn floats(
    piece: &VideoPiece,
    fps: Rational,
    size: (u32, u32),
    colors: super::renderer::Finish<'_>,
    decoder: Option<&crate::hw::GpuDecoder>,
) -> Result<Vec<[f32; 3]>> {
    crate::cancel::stop()?;
    crate::init();
    let pixel = format::Pixel::GBRPF32LE;
    let mut frames = super::renderer::frames(fps, size, pixel, colors, decoder)?.in_frame(size);
    frames.read_piece(piece);
    let frame = frames
        .next_frame()?
        .ok_or_else(|| Error::Invalid("no frame to render".into()))?;
    let (w, h) = (frame.width() as usize, frame.height() as usize);
    let value = |plane: usize, x: usize, y: usize| {
        let at = y * frame.stride(plane) + 4 * x;
        let bytes = &frame.data(plane)[at..at + 4];
        f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    };
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            out.push([value(2, x, y), value(0, x, y), value(1, x, y)]);
        }
    }
    Ok(out)
}

pub fn rgba(
    piece: &VideoPiece,
    fps: Rational,
    size: (u32, u32),
    colors: super::renderer::Finish<'_>,
) -> Result<crate::thumbs::Image> {
    super::renderer::FrameReader::default().rgba(piece, fps, size, colors)
}

pub fn png_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    crate::init();
    if rgba.len() != width as usize * height as usize * 4 || width == 0 || height == 0 {
        return Err(Error::Invalid(format!(
            "{width}x{height} RGBA needs {} bytes, got {}",
            width as usize * height as usize * 4,
            rgba.len()
        )));
    }
    encoded(crate::buffers::unpacked(
        format::Pixel::RGBA,
        (width, height),
        4,
        rgba,
    ))
}

fn encoded(mut frame: ffmpeg::frame::Video) -> Result<Vec<u8>> {
    frame.set_pts(Some(0));
    let codec = encoder::find_by_name("png")
        .ok_or_else(|| Error::Invalid("this FFmpeg has no PNG encoder".into()))?;
    let mut ctx = codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()
        .ctx(|| "creating PNG encoder".into())?;
    ctx.set_width(frame.width());
    ctx.set_height(frame.height());
    ctx.set_format(frame.format());
    ctx.set_time_base(ffmpeg::Rational(1, 25));
    let mut enc = ctx.open_as(codec).ctx(|| "opening PNG encoder".into())?;
    enc.send_frame(&frame).ctx(|| "encoding PNG".into())?;
    enc.send_eof().ctx(|| "encoding PNG".into())?;
    let mut packet = ffmpeg::Packet::empty();
    enc.receive_packet(&mut packet)
        .ctx(|| "encoding PNG".into())?;
    Ok(packet.data().unwrap_or_default().to_vec())
}

pub fn write_png(
    piece: &VideoPiece,
    fps: Rational,
    sizes: ((u32, u32), (u32, u32)),
    colors: super::renderer::Finish<'_>,
    path: &Path,
    decoder: Option<&crate::hw::GpuDecoder>,
) -> Result<()> {
    let bytes = png(piece, fps, sizes, colors, decoder)?;
    vidcut_core::temp::write(path, &bytes)?;
    Ok(())
}
