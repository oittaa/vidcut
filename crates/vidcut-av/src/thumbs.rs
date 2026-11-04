use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, codec, decoder, encoder, format, frame, software::scaling};

#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub(crate) fn of(f: &frame::Video) -> Self {
        Self {
            width: f.width(),
            height: f.height(),
            rgba: crate::buffers::packed(f, 4),
        }
    }
}

const MAGIC: &[u8; 4] = b"vct2";

const JPEG_QSCALE: i32 = 4;

pub fn to_bytes(images: &[Image], times: &[f64]) -> Result<Vec<u8>> {
    let first = images
        .first()
        .ok_or_else(|| Error::Invalid("no pictures to keep".into()))?;
    let (w, h) = (first.width, first.height);
    let size = w as usize * h as usize * 4;
    if times.len() != images.len()
        || images
            .iter()
            .any(|i| i.width != w || i.height != h || i.rgba.len() != size)
    {
        return Err(Error::Invalid(
            "a filmstrip's pictures must share one size and have a time each".into(),
        ));
    }
    let count = u32::try_from(images.len()).unwrap_or(u32::MAX);
    let strip_h = h
        .checked_mul(count)
        .filter(|&s| s <= 65_535)
        .ok_or_else(|| Error::Invalid("filmstrip too tall for a JPEG".into()))?;
    let rgba: Vec<u8> = images.iter().flat_map(|i| i.rgba.iter().copied()).collect();
    let jpeg = jpeg(w, strip_h, &rgba)?;
    let len = u32::try_from(jpeg.len()).map_err(|_| Error::Invalid("JPEG too large".into()))?;
    let mut out = Vec::with_capacity(20 + 8 * times.len() + jpeg.len());
    out.extend_from_slice(MAGIC);
    for v in [w, h, count, len] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for t in times {
        out.extend_from_slice(&t.to_le_bytes());
    }
    out.extend_from_slice(&jpeg);
    Ok(out)
}

pub fn from_bytes(bytes: &[u8]) -> Option<(Vec<Image>, Vec<f64>)> {
    let rest = bytes.strip_prefix(MAGIC)?;
    let (head, rest) = rest.split_first_chunk::<16>()?;
    let [w, h, n, len] =
        [0, 4, 8, 12].map(|i| u32::from_le_bytes(head[i..i + 4].try_into().expect("four bytes")));
    if w == 0 || h == 0 || n == 0 {
        return None;
    }
    let (times, jpeg) = rest.split_at_checked(8 * n as usize)?;
    if jpeg.len() != len as usize {
        return None;
    }
    let times: Vec<f64> = times
        .as_chunks::<8>()
        .0
        .iter()
        .map(|b| f64::from_le_bytes(*b))
        .collect();
    let rgba = unjpeg(jpeg, w, h.checked_mul(n)?)?;
    let images = rgba
        .chunks_exact(w as usize * h as usize * 4)
        .map(|c| Image {
            width: w,
            height: h,
            rgba: c.to_vec(),
        })
        .collect();
    Some((images, times))
}

fn jpeg(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    crate::init();
    let src = crate::buffers::unpacked(format::Pixel::RGBA, (width, height), 4, rgba);
    let mut yuv = frame::Video::empty();
    scaling::Context::get(
        format::Pixel::RGBA,
        width,
        height,
        format::Pixel::YUVJ420P,
        width,
        height,
        scaling::Flags::BICUBIC | scaling::Flags::ACCURATE_RND | scaling::Flags::FULL_CHR_H_INT,
    )
    .and_then(|mut s| s.run(&src, &mut yuv))
    .ctx(|| "converting thumbnails for JPEG".into())?;
    yuv.set_pts(Some(0));
    let codec = encoder::find(codec::Id::MJPEG)
        .ok_or_else(|| Error::Invalid("this FFmpeg has no JPEG encoder".into()))?;
    let mut ctx = codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()
        .ctx(|| "creating JPEG encoder".into())?;
    ctx.set_width(width);
    ctx.set_height(height);
    ctx.set_format(format::Pixel::YUVJ420P);
    ctx.set_time_base(ffmpeg::Rational(1, 25));
    let quality = JPEG_QSCALE * ffmpeg::ffi::FF_QP2LAMBDA;
    unsafe {
        let raw = ctx.as_mut_ptr();
        (*raw).flags |= ffmpeg::ffi::AV_CODEC_FLAG_QSCALE as i32;
        (*raw).global_quality = quality;
        (*raw).color_range = ffmpeg::ffi::AVColorRange::AVCOL_RANGE_JPEG;
        (*yuv.as_mut_ptr()).quality = quality;
    }
    let mut enc = ctx.open_as(codec).ctx(|| "opening JPEG encoder".into())?;
    enc.send_frame(&yuv).ctx(|| "encoding JPEG".into())?;
    enc.send_eof().ctx(|| "encoding JPEG".into())?;
    let mut packet = ffmpeg::Packet::empty();
    enc.receive_packet(&mut packet)
        .ctx(|| "encoding JPEG".into())?;
    Ok(packet.data().unwrap_or_default().to_vec())
}

fn unjpeg(jpeg: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    crate::init();
    let codec = decoder::find(codec::Id::MJPEG)?;
    let mut dec = codec::context::Context::new_with_codec(codec)
        .decoder()
        .video()
        .ok()?;
    dec.send_packet(&ffmpeg::Packet::copy(jpeg)).ok()?;
    dec.send_eof().ok()?;
    let mut f = frame::Video::empty();
    dec.receive_frame(&mut f).ok()?;
    if (f.width(), f.height()) != (width, height) {
        return None;
    }
    let full = unsafe { (*f.as_ptr()).color_range } == ffmpeg::ffi::AVColorRange::AVCOL_RANGE_JPEG;
    match f.format() {
        format::Pixel::YUV420P if full => f.set_format(format::Pixel::YUVJ420P),
        format::Pixel::YUV422P if full => f.set_format(format::Pixel::YUVJ422P),
        format::Pixel::YUV444P if full => f.set_format(format::Pixel::YUVJ444P),
        _ => {}
    }
    let mut rgba = frame::Video::empty();
    scaling::Context::get(
        f.format(),
        width,
        height,
        format::Pixel::RGBA,
        width,
        height,
        scaling::Flags::BICUBIC | scaling::Flags::ACCURATE_RND | scaling::Flags::FULL_CHR_H_INT,
    )
    .and_then(|mut s| s.run(&f, &mut rgba))
    .ok()?;
    Some(crate::buffers::packed(&rgba, 4))
}
