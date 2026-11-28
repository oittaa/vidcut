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
    Ok(floats_sized(piece, fps, size, colors, decoder)?.2)
}

fn float_frame(
    piece: &VideoPiece,
    fps: Rational,
    size: (u32, u32),
    colors: super::renderer::Finish<'_>,
    decoder: Option<&crate::hw::GpuDecoder>,
) -> Result<ffmpeg::frame::Video> {
    crate::cancel::stop()?;
    crate::init();
    let pixel = format::Pixel::GBRPF32LE;
    let mut frames = super::renderer::frames(fps, size, pixel, colors, decoder)?.in_frame(size);
    frames.read_piece(piece);
    frames
        .next_frame()?
        .ok_or_else(|| Error::Invalid("no frame to render".into()))
}

pub fn floats_sized(
    piece: &VideoPiece,
    fps: Rational,
    size: (u32, u32),
    colors: super::renderer::Finish<'_>,
    decoder: Option<&crate::hw::GpuDecoder>,
) -> Result<(u32, u32, Vec<[f32; 3]>)> {
    let frame = float_frame(piece, fps, size, colors, decoder)?;
    Ok(floats_of(&frame))
}

pub(crate) fn floats_of(frame: &crate::ffmpeg::frame::Video) -> (u32, u32, Vec<[f32; 3]>) {
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
    (frame.width(), frame.height(), out)
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

pub fn write_exr(
    piece: &VideoPiece,
    fps: Rational,
    size: (u32, u32),
    colors: super::renderer::Finish<'_>,
    path: &Path,
    decoder: Option<&crate::hw::GpuDecoder>,
) -> Result<()> {
    let frame = float_frame(piece, fps, size, colors, decoder)?;
    let (w, h) = (frame.width(), frame.height());
    let row = w as usize * 4;
    let mut planes = Vec::with_capacity(row * h as usize * 3);
    for plane in 0..3 {
        let (stride, data) = (frame.stride(plane), frame.data(plane));
        for y in 0..h as usize {
            planes.extend_from_slice(&data[y * stride..y * stride + row]);
        }
    }
    let (temp, file) = vidcut_core::temp::file(path)?;
    drop(file);
    super::openexr::Writer::still((w, h), fps).write(&planes, None, temp.path())?;
    temp.keep(path)?;
    Ok(())
}

struct Shrunk {
    size: (usize, usize),
    from: (usize, usize),
    sums: Vec<[f32; 4]>,
}

pub fn exr_rgba(path: &Path, height: u32) -> Result<crate::thumbs::Image> {
    use ::exr::prelude::{RgbaChannels, Vec2, read_first_rgba_layer_from_file};
    let tall = height.max(1) as usize;
    let image = read_first_rgba_layer_from_file(
        path,
        move |from: Vec2<usize>, _: &RgbaChannels| {
            let h = tall.min(from.1.max(1));
            let w = (from.0 * h).div_ceil(from.1.max(1)).max(1);
            Shrunk {
                size: (w, h),
                from: (from.0.max(1), from.1.max(1)),
                sums: vec![[0.0; 4]; w * h],
            }
        },
        |s: &mut Shrunk, at: Vec2<usize>, (r, g, b, _): (f32, f32, f32, f32)| {
            let x = at.0 * s.size.0 / s.from.0;
            let y = at.1 * s.size.1 / s.from.1;
            let sum = &mut s.sums[y * s.size.0 + x];
            for (k, v) in [r, g, b].into_iter().enumerate() {
                sum[k] += if v.is_finite() { v } else { 0.0 };
            }
            sum[3] += 1.0;
        },
    )
    .map_err(|e| Error::Invalid(format!("reading {}: {e}", path.display())))?;
    let s = image.layer_data.channel_data.pixels;
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut rgba = Vec::with_capacity(s.sums.len() * 4);
    for [r, g, b, n] in s.sums {
        let n = n.max(1.0);
        rgba.extend([byte(r / n), byte(g / n), byte(b / n), 255]);
    }
    Ok(crate::thumbs::Image {
        width: s.size.0 as u32,
        height: s.size.1 as u32,
        rgba,
    })
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
