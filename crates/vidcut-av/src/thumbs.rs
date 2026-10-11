//! Small pictures for the media pool: frames spread evenly over a clip (a
//! "filmstrip"), used as the poster and for hover scrubbing (spec §3).
//!
//! A picture may be up to half the spacing between pictures early: for long
//! clips that is usually the keyframe at or before its time (one seek, one
//! decoded frame), and for short clips with sparse keyframes decoding simply
//! continues forward from the previous picture. Either way the cost stays
//! close to decoding one frame per picture or the clip once.

use std::path::Path;

use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, codec, decoder, encoder, format, frame, media, software::scaling};

/// An RGBA picture, rows top to bottom, no padding.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// `count` pictures at evenly spaced times of `path`'s video (the middle of
/// each of `count` equal parts), each fitted into `max_w`×`max_h` keeping the
/// display aspect. Returns the pictures and their times in seconds from the
/// first frame.
pub fn filmstrip(
    path: &Path,
    count: usize,
    max_w: u32,
    max_h: u32,
) -> Result<(Vec<Image>, Vec<f64>)> {
    crate::init();
    let mut input = format::input(path).ctx(|| format!("opening {}", path.display()))?;
    let stream = input
        .streams()
        .best(media::Type::Video)
        .ok_or_else(|| Error::Invalid(format!("{}: no video stream", path.display())))?;
    let index = stream.index();
    let tb = f64::from(stream.time_base());
    let start = Some(stream.start_time())
        .filter(|&t| t != ffmpeg::ffi::AV_NOPTS_VALUE)
        .map_or(0.0, |t| t as f64 * tb);
    let duration = match stream.duration() {
        d if d > 0 => d as f64 * tb,
        _ => input.duration().max(0) as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE),
    };
    let mut ctx = codec::context::Context::from_parameters(stream.parameters())
        .ctx(|| "creating video decoder context".into())?;
    unsafe {
        let raw = ctx.as_mut_ptr();
        (*raw).pkt_timebase = stream.time_base().into();
        // Thumbnails don't need B-frames or deblocking; skipping them makes
        // decoding forward through short clips about twice as fast.
        (*raw).skip_frame = ffmpeg::ffi::AVDiscard::AVDISCARD_NONREF;
        (*raw).skip_loop_filter = ffmpeg::ffi::AVDiscard::AVDISCARD_ALL;
    }
    let mut decoder = ctx
        .decoder()
        .video()
        .ctx(|| "opening video decoder".into())?;
    crate::export::audio::discard_others(&input, index);

    let rotation =
        crate::decode::stream_rotation(&input.stream(index).expect("found").parameters());
    let (w, h) = (decoder.width().max(1), decoder.height().max(1));
    let sar = decoder.aspect_ratio();
    let stored_aspect = if sar.numerator() > 0 && sar.denominator() > 0 {
        f64::from(w) * f64::from(sar.numerator()) / f64::from(sar.denominator()) / f64::from(h)
    } else {
        f64::from(w) / f64::from(h)
    };
    // Fitted as shown; scaled as stored, then turned upright.
    let quarter = rotation % 180 == 90;
    let aspect = if quarter {
        1.0 / stored_aspect
    } else {
        stored_aspect
    };
    let (tw, th) = fit(aspect, max_w, max_h);
    let (tw, th) = if quarter { (th, tw) } else { (tw, th) };
    let mut scaler: Option<scaling::Context> = None;

    let spacing = duration / count.max(1) as f64;
    let mut images = Vec::with_capacity(count);
    let mut times = Vec::with_capacity(count);
    // Time (from `start`) of the last frame decoded, while decoding forward.
    let mut position: Option<f64> = None;
    for i in 0..count {
        let t = duration * (i as f64 + 0.5) / count as f64;
        let near_ahead = position.is_some_and(|p| p <= t && t - p <= 3.0);
        if !near_ahead {
            // Keyframe at or before the target; demuxers that can't do that
            // land somewhere close, which is fine for a thumbnail.
            let ts = ((start + t) * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
            let _ = input.seek(ts, ..ts);
            decoder.flush();
        }
        let mut picked = None;
        for decoded_frames in 0.. {
            let Some(f) = next_frame(&mut input, &mut decoder, index)? else {
                break;
            };
            let ft = f.timestamp().map_or(t, |ts| ts as f64 * tb - start);
            position = Some(ft);
            let done = ft >= t - spacing / 2.0 || decoded_frames >= 300;
            picked = Some(f);
            if done {
                break;
            }
        }
        let Some(decoded) = picked else { break };
        let scaler = match &mut scaler {
            Some(s) if s.input().format == decoded.format() => s,
            _ => scaler.insert(
                scaling::Context::get(
                    decoded.format(),
                    decoded.width(),
                    decoded.height(),
                    format::Pixel::RGBA,
                    tw,
                    th,
                    scaling::Flags::BILINEAR,
                )
                .ctx(|| "creating thumbnail scaler".into())?,
            ),
        };
        let mut rgba = frame::Video::empty();
        scaler
            .run(&decoded, &mut rgba)
            .ctx(|| "scaling thumbnail".into())?;
        images.push(turn(packed(&rgba), rotation));
        times.push(t);
    }
    Ok((images, times))
}

/// The start of a filmstrip's cache form.
const MAGIC: &[u8; 4] = b"vct1";

/// JPEG quantizer scale for cached filmstrips (2 is best, 31 worst): about
/// 6 KB a picture, where RGBA takes 83 KB, with no visible loss at
/// thumbnail size.
const JPEG_QSCALE: i32 = 4;

/// A filmstrip's cache form: a header (magic; picture width, height and
/// count; the JPEG's length; the times) followed by the pictures as one
/// JPEG, stacked top to bottom. The length tells a file cut short, which
/// JPEG decoders otherwise show half gray.
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

/// The pictures and times of a filmstrip's cache form; `None` if `bytes`
/// isn't one.
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

/// Encodes packed RGBA as a full-range 4:2:0 JPEG.
fn jpeg(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    crate::init();
    let mut src = frame::Video::new(format::Pixel::RGBA, width, height);
    let stride = src.stride(0);
    let row = width as usize * 4;
    for (y, line) in rgba.chunks_exact(row).enumerate() {
        src.data_mut(0)[y * stride..y * stride + row].copy_from_slice(line);
    }
    let mut yuv = frame::Video::empty();
    scaling::Context::get(
        format::Pixel::RGBA,
        width,
        height,
        format::Pixel::YUVJ420P,
        width,
        height,
        scaling::Flags::BICUBIC,
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
        // A fixed quantizer comes from each frame.
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

/// Decodes a JPEG of exactly `width`×`height` to packed RGBA.
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
    // Full-range YUV may come as the plain formats with the range set;
    // swscale takes the range from the J formats.
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
        scaling::Flags::BICUBIC,
    )
    .and_then(|mut s| s.run(&f, &mut rgba))
    .ok()?;
    Some(packed(&rgba).rgba)
}

/// `img` turned clockwise by `rotation` (0, 90, 180 or 270 degrees).
fn turn(img: Image, rotation: u16) -> Image {
    if rotation == 0 {
        return img;
    }
    let (w, h) = (img.width as usize, img.height as usize);
    let (tw, th) = if rotation == 180 { (w, h) } else { (h, w) };
    let mut rgba = vec![0; img.rgba.len()];
    for y in 0..h {
        for x in 0..w {
            let (tx, ty) = match rotation {
                90 => (h - 1 - y, x),
                180 => (w - 1 - x, h - 1 - y),
                _ => (y, w - 1 - x),
            };
            let (s, d) = (4 * (y * w + x), 4 * (ty * tw + tx));
            rgba[d..d + 4].copy_from_slice(&img.rgba[s..s + 4]);
        }
    }
    Image {
        width: tw as u32,
        height: th as u32,
        rgba,
    }
}

/// The largest even size with `aspect` that fits `max_w`×`max_h`.
fn fit(aspect: f64, max_w: u32, max_h: u32) -> (u32, u32) {
    let (mw, mh) = (f64::from(max_w), f64::from(max_h));
    let (w, h) = if mw / mh > aspect {
        (mh * aspect, mh)
    } else {
        (mw, mw / aspect)
    };
    let even = |v: f64| ((v.round() as u32) & !1).max(2);
    (even(w), even(h))
}

/// Decodes the next frame (`None` at the end of the stream).
fn next_frame(
    input: &mut format::context::Input,
    decoder: &mut ffmpeg::decoder::Video,
    index: usize,
) -> Result<Option<frame::Video>> {
    let mut eof = false;
    loop {
        let mut f = frame::Video::empty();
        match decoder.receive_frame(&mut f) {
            Ok(()) => return Ok(Some(f)),
            Err(ffmpeg::Error::Eof) => return Ok(None),
            Err(ffmpeg::Error::Other {
                errno: ffmpeg::util::error::EAGAIN,
            }) => {}
            // Decoders complain about missing references right after a seek.
            Err(_) => {}
        }
        if eof {
            return Ok(None);
        }
        loop {
            let mut packet = ffmpeg::Packet::empty();
            match packet.read(input) {
                Ok(()) if packet.stream() == index => {
                    let _ = decoder.send_packet(&packet);
                    break;
                }
                Ok(()) => continue,
                Err(ffmpeg::Error::Eof) => {
                    let _ = decoder.send_eof();
                    eof = true;
                    break;
                }
                Err(e) => {
                    return Err(Error::Ffmpeg {
                        context: "reading packet".into(),
                        source: e,
                    });
                }
            }
        }
    }
}

/// Copies an RGBA frame into a tightly packed buffer.
fn packed(f: &frame::Video) -> Image {
    let (w, h) = (f.width(), f.height());
    let stride = f.stride(0);
    let row = w as usize * 4;
    let data = f.data(0);
    let mut rgba = Vec::with_capacity(row * h as usize);
    for y in 0..h as usize {
        rgba.extend_from_slice(&data[y * stride..y * stride + row]);
    }
    Image {
        width: w,
        height: h,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_form_keeps_sizes_times_and_pictures() {
        // Three 64x36 gradients, each a different hue.
        let images: Vec<Image> = (0..3u8)
            .map(|k| {
                let mut rgba = Vec::new();
                for y in 0..36u8 {
                    for x in 0..64u8 {
                        rgba.extend_from_slice(&[x * 4, y * 7, 80 * k, 255]);
                    }
                }
                Image {
                    width: 64,
                    height: 36,
                    rgba,
                }
            })
            .collect();
        let times = [0.5, 1.5, 2.25];
        let bytes = to_bytes(&images, &times).unwrap();
        assert!(
            bytes.len() < images.len() * 64 * 36,
            "{} bytes",
            bytes.len()
        );
        let (back, back_times) = from_bytes(&bytes).unwrap();
        assert_eq!(back_times, times);
        assert_eq!(back.len(), 3);
        for (a, b) in images.iter().zip(&back) {
            assert_eq!((b.width, b.height, b.rgba.len()), (64, 36, a.rgba.len()));
            let diff = a
                .rgba
                .iter()
                .zip(&b.rgba)
                .map(|(x, y)| u64::from(x.abs_diff(*y)))
                .sum::<u64>() as f64
                / a.rgba.len() as f64;
            assert!(diff < 3.0, "mean difference {diff}");
        }
        assert!(from_bytes(&bytes[..bytes.len() / 2]).is_none(), "cut short");
        assert!(from_bytes(b"vct1").is_none());
        assert!(from_bytes(&[b"xxxx".as_slice(), &bytes[4..]].concat()).is_none());
        assert!(to_bytes(&images, &times[..2]).is_err(), "a time each");
    }

    #[test]
    fn turning_moves_the_top_left_corner_where_a_rotation_does() {
        // A 3x2 picture whose pixels are numbered 0..6 in the red channel.
        let img = Image {
            width: 3,
            height: 2,
            rgba: (0..6u8).flat_map(|i| [i, 0, 0, 255]).collect(),
        };
        let red = |i: &Image| i.rgba.iter().step_by(4).copied().collect::<Vec<u8>>();
        let q = turn(img.clone(), 90);
        assert_eq!((q.width, q.height), (2, 3));
        // Clockwise: the bottom row becomes the left column.
        assert_eq!(red(&q), [3, 0, 4, 1, 5, 2]);
        assert_eq!(red(&turn(img.clone(), 180)), [5, 4, 3, 2, 1, 0]);
        assert_eq!(red(&turn(img.clone(), 270)), [2, 5, 1, 4, 0, 3]);
        assert_eq!(turn(turn(img.clone(), 90), 270), img);
    }

    #[test]
    fn fit_keeps_the_aspect_and_even_sizes() {
        assert_eq!(fit(16.0 / 9.0, 192, 108), (192, 108));
        assert_eq!(fit(2.4, 192, 108), (192, 80));
        assert_eq!(fit(9.0 / 16.0, 192, 108), (60, 108));
    }
}
