//! Still frames as PNG: what the timeline (or a media file) shows at a frame,
//! picked by the same sampling rule as export. Lets agents look at a cut.

use std::path::Path;

use vidcut_core::render::VideoPiece;
use vidcut_core::time::Rational;

use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, codec, encoder, format};

use super::video::VideoFrames;

/// Renders one video piece frame (a single-frame piece) as PNG bytes at
/// `width`×`height` (the picture fit inside, padded black).
pub fn png(piece: &VideoPiece, fps: Rational, width: u32, height: u32) -> Result<Vec<u8>> {
    crate::cancel::stop()?;
    crate::init();
    let mut frames = VideoFrames::new(
        std::slice::from_ref(piece),
        fps,
        width,
        height,
        format::Pixel::RGB24,
        None,
    );
    let mut frame = frames
        .next_frame()?
        .ok_or_else(|| Error::Invalid("no frame to render".into()))?;
    frame.set_pts(Some(0));
    let codec = encoder::find_by_name("png")
        .ok_or_else(|| Error::Invalid("this FFmpeg has no PNG encoder".into()))?;
    let mut ctx = codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()
        .ctx(|| "creating PNG encoder".into())?;
    ctx.set_width(width);
    ctx.set_height(height);
    ctx.set_format(format::Pixel::RGB24);
    ctx.set_time_base(ffmpeg::Rational(1, 25));
    let mut enc = ctx.open_as(codec).ctx(|| "opening PNG encoder".into())?;
    enc.send_frame(&frame).ctx(|| "encoding PNG".into())?;
    enc.send_eof().ctx(|| "encoding PNG".into())?;
    let mut packet = ffmpeg::Packet::empty();
    enc.receive_packet(&mut packet)
        .ctx(|| "encoding PNG".into())?;
    Ok(packet.data().unwrap_or_default().to_vec())
}

/// Renders one video piece frame (a single-frame piece) as packed RGBA at
/// `width`×`height` (the picture fit inside, padded black).
pub fn rgba(
    piece: &VideoPiece,
    fps: Rational,
    width: u32,
    height: u32,
) -> Result<crate::thumbs::Image> {
    let img = super::renderer::StillRenderer::with_budget(0).rgba(piece, fps, width, height)?;
    Ok(std::sync::Arc::try_unwrap(img).unwrap_or_else(|img| (*img).clone()))
}

/// Encodes a packed RGBA picture (rows top to bottom) as PNG.
pub fn png_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    crate::init();
    if rgba.len() != width as usize * height as usize * 4 || width == 0 || height == 0 {
        return Err(Error::Invalid(format!(
            "{width}x{height} RGBA needs {} bytes, got {}",
            width as usize * height as usize * 4,
            rgba.len()
        )));
    }
    let mut frame = ffmpeg::frame::Video::new(format::Pixel::RGBA, width, height);
    let stride = frame.stride(0);
    let row = width as usize * 4;
    for (y, src) in rgba.chunks_exact(row).enumerate() {
        frame.data_mut(0)[y * stride..y * stride + row].copy_from_slice(src);
    }
    frame.set_pts(Some(0));
    let codec = encoder::find_by_name("png")
        .ok_or_else(|| Error::Invalid("this FFmpeg has no PNG encoder".into()))?;
    let mut ctx = codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()
        .ctx(|| "creating PNG encoder".into())?;
    ctx.set_width(width);
    ctx.set_height(height);
    ctx.set_format(format::Pixel::RGBA);
    ctx.set_time_base(ffmpeg::Rational(1, 25));
    let mut enc = ctx.open_as(codec).ctx(|| "opening PNG encoder".into())?;
    enc.send_frame(&frame).ctx(|| "encoding PNG".into())?;
    enc.send_eof().ctx(|| "encoding PNG".into())?;
    let mut packet = ffmpeg::Packet::empty();
    enc.receive_packet(&mut packet)
        .ctx(|| "encoding PNG".into())?;
    Ok(packet.data().unwrap_or_default().to_vec())
}

/// Writes a PNG of one piece frame to `path`.
pub fn write_png(
    piece: &VideoPiece,
    fps: Rational,
    width: u32,
    height: u32,
    path: &Path,
) -> Result<()> {
    let bytes = png(piece, fps, width, height)?;
    std::fs::write(path, bytes)?;
    Ok(())
}
