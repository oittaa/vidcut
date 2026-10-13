//! Small pictures for the media pool: frames spread evenly over a clip (a
//! "filmstrip"), used as the poster and for hover scrubbing (spec §3).
//!
//! A picture may be up to half the spacing between pictures early: for long
//! clips that is usually the keyframe at or before its time (one seek, one
//! decoded frame), and for short clips with sparse keyframes decoding simply
//! continues forward from the previous picture. Either way the cost stays
//! close to decoding one frame per picture or the clip once.

use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, codec, decoder, encoder, format, frame, media, software::scaling};

/// An RGBA picture, rows top to bottom, no padding.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// `count` pictures at evenly spaced times of `source`'s video (the middle
/// of each of `count` equal parts), each fitted into `max_w`×`max_h` keeping
/// the display aspect. Returns the pictures and their times in seconds from
/// the first frame. A still gives its one picture.
///
/// An image sequence is read by FFmpeg's image2 demuxer, which continues
/// while the file numbers do. Pass the sequence's own length to
/// [`filmstrip_pictures`].
pub fn filmstrip(
    source: &crate::decode::Input,
    count: usize,
    max_w: u32,
    max_h: u32,
) -> Result<(Vec<Image>, Vec<f64>)> {
    filmstrip_inner(source, count, max_w, max_h, None)
}

/// [`filmstrip`] of the first `pictures` frames of `source`.
///
/// `pictures` is an image sequence's length. image2 does not take it, so a
/// sequence that is only the start of a longer numbered run would otherwise
/// last as long as that run, and the strip would show the later files. A
/// sample past the last of these pictures stays on the last one: the next
/// file is not opened, and a missing next file does not fail the strip.
pub fn filmstrip_pictures(
    source: &crate::decode::Input,
    pictures: u64,
    count: usize,
    max_w: u32,
    max_h: u32,
) -> Result<(Vec<Image>, Vec<f64>)> {
    filmstrip_inner(source, count, max_w, max_h, Some(pictures))
}

fn filmstrip_inner(
    source: &crate::decode::Input,
    count: usize,
    max_w: u32,
    max_h: u32,
    pictures: Option<u64>,
) -> Result<(Vec<Image>, Vec<f64>)> {
    if pictures == Some(0) {
        return Ok((Vec::new(), Vec::new()));
    }
    let path = source.path();
    let mut input = source.open()?;
    let stream = input
        .streams()
        .best(media::Type::Video)
        .ok_or_else(|| Error::Invalid(format!("{}: no video stream", path.display())))?;
    let index = stream.index();
    let tb = f64::from(stream.time_base());
    let start = Some(stream.start_time())
        .filter(|&t| t != ffmpeg::ffi::AV_NOPTS_VALUE)
        .map_or(0.0, |t| t as f64 * tb);
    let mut duration = match stream.duration() {
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
    let sar = (sar.numerator(), sar.denominator());
    // Fitted as shown (turned upright, square pixels), as export fits.
    let (tw, th) = if rotation % 180 == 90 {
        crate::export::video::fit(h, w, (sar.1, sar.0), max_w, max_h)
    } else {
        crate::export::video::fit(w, h, sar, max_w, max_h)
    };
    // Export's normalizer: upright, tone-mapped to SDR, as RGBA.
    let mut normalizer: Option<crate::export::video::Normalizer> = None;

    // image2's duration counts every numbered file after the first. A
    // sequence is only its own pictures, at `rate`.
    let rate = stream_rate(&input.stream(index).expect("found"));
    let pictures = pictures.filter(|n| *n > 0);
    if let Some(n) = pictures
        && rate > 0.0
    {
        let span = n as f64 / rate;
        duration = if duration > 0.0 {
            duration.min(span)
        } else {
            span
        };
    }
    let spacing = duration / count.max(1) as f64;
    let mut images = Vec::with_capacity(count);
    let mut times = Vec::with_capacity(count);
    // Time (from `start`) of the last frame decoded, while decoding forward.
    let mut position: Option<f64> = None;
    for i in 0..count {
        let t = duration * (i as f64 + 0.5) / count as f64;
        // The picture a sequence sample shows. Seeking by a rounded
        // timestamp names the next file, which is not part of the sequence
        // and may not exist; that open fails the whole strip.
        if let Some(n) = pictures
            && rate > 0.0
        {
            let k = ((t * rate).floor().max(0.0) as u64).min(n - 1);
            match one_sequence_picture(&mut input, &mut decoder, index, k) {
                Some(decoded) => keep(
                    &mut images,
                    &mut times,
                    &mut normalizer,
                    &decoded,
                    (rotation, tw, th),
                    t,
                )?,
                None => {
                    if let Some(prev) = images.last() {
                        images.push(prev.clone());
                        times.push(t);
                        continue;
                    }
                    break;
                }
            }
            continue;
        }
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
        keep(
            &mut images,
            &mut times,
            &mut normalizer,
            &decoded,
            (rotation, tw, th),
            t,
        )?;
    }
    Ok((images, times))
}

/// Appends one fitted picture and the sample time it stands for.
fn keep(
    images: &mut Vec<Image>,
    times: &mut Vec<f64>,
    normalizer: &mut Option<crate::export::video::Normalizer>,
    decoded: &frame::Video,
    place: (u16, u32, u32),
    t: f64,
) -> Result<()> {
    let (rotation, tw, th) = place;
    let n = match normalizer {
        Some(n) if n.matches(decoded, rotation) => n,
        _ => normalizer.insert(crate::export::video::Normalizer::new(
            decoded,
            rotation,
            tw,
            th,
            format::Pixel::RGBA,
            None,
        )?),
    };
    images.push(packed(&n.run(decoded)?));
    times.push(t);
    Ok(())
}

/// Frames per second of `stream`, from its average rate or its time base.
fn stream_rate(stream: &format::stream::Stream) -> f64 {
    let avg = f64::from(stream.avg_frame_rate());
    if avg.is_finite() && avg > 0.0 {
        return avg;
    }
    let tb = f64::from(stream.time_base());
    if tb > 0.0 { 1.0 / tb } else { 0.0 }
}

/// Picture `index` of an image sequence (0 is its first file), and nothing
/// after it.
///
/// image2's seek takes a frame index. One packet is that one file: the
/// packet after it would be the next number, which a sequence does not
/// include. `None` when the picture is not there — a sample past the
/// sequence must not fail the strip, and must not open a path it does not
/// have.
fn one_sequence_picture(
    input: &mut format::context::Input,
    decoder: &mut ffmpeg::decoder::Video,
    stream: usize,
    index: u64,
) -> Option<frame::Video> {
    decoder.flush();
    let found = unsafe {
        ffmpeg::ffi::avformat_seek_file(
            input.as_mut_ptr(),
            stream as i32,
            i64::MIN,
            index as i64,
            index as i64,
            0,
        ) >= 0
    };
    decoder.flush();
    if !found {
        return None;
    }
    let mut packet = ffmpeg::Packet::empty();
    loop {
        match packet.read(input) {
            Ok(()) if packet.stream() == stream => break,
            Ok(()) => continue,
            Err(_) => return None,
        }
    }
    if decoder.send_packet(&packet).is_err() {
        return None;
    }
    let mut frame = frame::Video::empty();
    match decoder.receive_frame(&mut frame) {
        Ok(()) => Some(frame),
        Err(ffmpeg::Error::Other {
            errno: ffmpeg::util::error::EAGAIN,
        }) => {
            // A picture decoder can hold its frame until the stream ends.
            if decoder.send_eof().is_err() {
                return None;
            }
            decoder.receive_frame(&mut frame).ok().map(|()| frame)
        }
        Err(_) => None,
    }
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
        // Exact rounding: by default white came back 2 levels green.
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
        // Exact rounding: by default white came back 2 levels green.
        scaling::Flags::BICUBIC | scaling::Flags::ACCURATE_RND | scaling::Flags::FULL_CHR_H_INT,
    )
    .and_then(|mut s| s.run(&f, &mut rgba))
    .ok()?;
    Some(packed(&rgba).rgba)
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

    /// Through the JPEG of the cache and back, white stays white and gray
    /// gray (swscale's default rounding brought them back 2 levels green).
    #[test]
    fn the_cache_form_keeps_levels() {
        let (w, h) = (64, 32);
        let colors = [
            [255, 255, 255, 255],
            [128, 128, 128, 255],
            [0, 0, 0, 255],
            [191, 0, 0, 255],
        ];
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|i| colors[(i % w) * colors.len() / w])
            .collect();
        let back = unjpeg(
            &jpeg(w as u32, h as u32, &rgba).unwrap(),
            w as u32,
            h as u32,
        )
        .unwrap();
        for (k, want) in colors.iter().enumerate() {
            // The middle of band k, in the middle row.
            let x = (2 * k + 1) * w / (2 * colors.len());
            let got = &back[(h / 2 * w + x) * 4..][..3];
            // Grays exact; JPEG's quantizing may move red's channels a level.
            let near = if k < 3 { 0 } else { 1 };
            assert!(
                got.iter().zip(want).all(|(g, w)| g.abs_diff(*w) <= near),
                "{want:?} came back {got:?}"
            );
        }
    }

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

    /// Barcode pictures numbered from `first` in `dir`. Picture `k` shows
    /// barcode `k`. Returns the image2 pattern.
    fn numbered_pngs(dir: &std::path::Path, first: u32, n: u32) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let (w, h) = (320u32, 180u32);
        let mut luma = vec![0u8; w as usize * h as usize];
        for k in 0..n {
            crate::barcode::paint(k, &mut luma, w as usize, h as usize, w as usize);
            let rgba: Vec<u8> = luma.iter().flat_map(|&y| [y, y, y, 255]).collect();
            let png = crate::export::still::png_rgba(w, h, &rgba).unwrap();
            std::fs::write(dir.join(format!("img_{:04}.png", first + k)), png).unwrap();
        }
        dir.join("img_%04d.png")
    }

    fn codes(images: &[Image]) -> Vec<u32> {
        images
            .iter()
            .map(|img| {
                crate::barcode::read_packed(
                    &img.rgba,
                    img.width as usize,
                    img.height as usize,
                    img.width as usize * 4,
                    4,
                    1,
                )
                .unwrap_or_else(|| panic!("{}x{} unreadable", img.width, img.height))
            })
            .collect()
    }

    #[test]
    fn a_sequence_prefix_stops_at_its_count() {
        // Eight numbered files. The clip is the first four: image2 would
        // keep going, and the strip would show the later pictures. A sample
        // past those four must not open the next path (it is removed below)
        // and must not fail the strip.
        let tmp = tempfile::Builder::new()
            .prefix("vidcut-seq-")
            .tempdir()
            .unwrap();
        let dir = tmp.path();
        let pattern = numbered_pngs(dir, 1000, 8);
        let input = crate::decode::Input::Sequence {
            pattern,
            first: 1000,
            rate: (24, 1),
        };
        let (wide, wide_times) = filmstrip(&input, 4, 320, 180).unwrap();
        let wide_codes = codes(&wide);
        assert!(
            wide_codes.iter().any(|c| *c >= 4),
            "the longer run is visible without a count: {wide_codes:?} at {wide_times:?}"
        );
        let (images, times) = filmstrip_pictures(&input, 4, 4, 320, 180).unwrap();
        assert_eq!(codes(&images), vec![0, 1, 2, 3], "{times:?}");
        assert!(
            *times.last().unwrap() < 4.0 / 24.0,
            "spread over the four pictures, not the eight files: {times:?}"
        );
        let stats = crate::analysis::picture_stats(&input, Some(4)).unwrap();
        assert_eq!(stats.pts.len(), 4);
        assert_eq!(
            crate::analysis::picture_stats(&input, None)
                .unwrap()
                .pts
                .len(),
            8,
            "without a count, image2 measures the files that follow"
        );

        for k in 4..8 {
            std::fs::remove_file(dir.join(format!("img_{:04}.png", 1000 + k))).unwrap();
        }
        let (tail, _) = filmstrip_pictures(&input, 4, 24, 320, 180).unwrap();
        assert_eq!(tail.len(), 24);
        assert!(codes(&tail).iter().all(|c| *c < 4), "{:?}", codes(&tail));
    }
}
