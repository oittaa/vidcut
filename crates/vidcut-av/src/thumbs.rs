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
use crate::ffmpeg::{self, codec, format, frame, media, software::scaling};

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

    let (w, h) = (decoder.width().max(1), decoder.height().max(1));
    let sar = decoder.aspect_ratio();
    let aspect = if sar.numerator() > 0 && sar.denominator() > 0 {
        f64::from(w) * f64::from(sar.numerator()) / f64::from(sar.denominator()) / f64::from(h)
    } else {
        f64::from(w) / f64::from(h)
    };
    let (tw, th) = fit(aspect, max_w, max_h);
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
        images.push(packed(&rgba));
        times.push(t);
    }
    Ok((images, times))
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
    fn fit_keeps_the_aspect_and_even_sizes() {
        assert_eq!(fit(16.0 / 9.0, 192, 108), (192, 108));
        assert_eq!(fit(2.4, 192, 108), (192, 80));
        assert_eq!(fit(9.0 / 16.0, 192, 108), (60, 108));
    }
}
