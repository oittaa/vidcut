//! Playback proxies: an easy-to-decode H.264 copy of a source's pictures.
//!
//! Each proxy frame keeps the source frame's presentation timestamp, in the
//! source time base when that base is ordinary and in microseconds otherwise.
//! The preview can then swap the file and keep the same EDL `start` math
//! (decision D41). Audio is not copied: preview audio stays on the original.
//!
//! The file is Matroska, written to a temporary name and renamed into place.
//! Pictures are turned upright and scaled to `width`×`height` as 8-bit 4:2:0.

use std::collections::HashMap;
use std::path::Path;

use crate::decode::{Input, VideoReader};
use crate::error::{Context, Error, Result};
use crate::export::video::Normalizer;
use crate::ffmpeg::{self, codec, encoder, format, frame};

/// What [`write`] produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenProxy {
    pub path: std::path::PathBuf,
    pub width: u32,
    pub height: u32,
    /// Frames written.
    pub frames: u64,
    pub codec: &'static str,
}

/// Encodes `input`'s pictures to `dest` at `width`×`height` (even).
pub fn write(input: &Input, width: u32, height: u32, dest: &Path) -> Result<WrittenProxy> {
    crate::init();
    if width < 2 || height < 2 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(Error::Invalid(format!(
            "proxy size {width}x{height} must be even and at least 2"
        )));
    }
    let mut reader = VideoReader::open_input(input)?;
    let src_tb = reader.time_base();
    let enc_tb = encode_time_base(src_tb);
    let rotation = reader.rotation();
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Invalid(format!("creating {}: {e}", parent.display())))?;
    }
    let tmp = dest.with_extension("part.mkv");
    let result = encode_into(&mut reader, width, height, rotation, src_tb, enc_tb, &tmp);
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    let frames = result?;
    std::fs::rename(&tmp, dest)
        .map_err(|e| Error::Invalid(format!("replacing {}: {e}", dest.display())))?;
    Ok(WrittenProxy {
        path: dest.to_path_buf(),
        width,
        height,
        frames,
        codec: "h264",
    })
}

/// The encoder's time base: the source's, when its denominator is a sane
/// clock, otherwise microseconds. Either way a frame's timestamp in seconds
/// matches the source.
fn encode_time_base(src: ffmpeg::Rational) -> ffmpeg::Rational {
    let (n, d) = (src.numerator(), src.denominator());
    if n > 0 && (1..=1_000_000).contains(&d) {
        src
    } else {
        ffmpeg::Rational(1, 1_000_000)
    }
}

fn scale_pts(pts: i64, from: ffmpeg::Rational, to: ffmpeg::Rational) -> i64 {
    if from == to {
        return pts;
    }
    let n = i128::from(pts) * i128::from(from.numerator()) * i128::from(to.denominator());
    let d = i128::from(from.denominator().max(1)) * i128::from(to.numerator().max(1));
    (n / d) as i64
}

fn encode_into(
    reader: &mut VideoReader,
    width: u32,
    height: u32,
    rotation: u16,
    src_tb: ffmpeg::Rational,
    enc_tb: ffmpeg::Rational,
    tmp: &Path,
) -> Result<u64> {
    let mut octx =
        format::output_as(tmp, "matroska").ctx(|| format!("creating {}", tmp.display()))?;
    let codec = encoder::find_by_name("libx264")
        .ok_or_else(|| Error::Invalid("this FFmpeg has no libx264 encoder".into()))?;
    let global = octx.format().flags().contains(format::Flags::GLOBAL_HEADER);
    let mut ctx = codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()
        .ctx(|| "creating the proxy encoder".into())?;
    ctx.set_width(width);
    ctx.set_height(height);
    ctx.set_format(format::Pixel::YUV420P);
    ctx.set_time_base(enc_tb);
    ctx.set_gop(60);
    ctx.set_max_b_frames(0);
    if global {
        ctx.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    crate::color::tag(unsafe { ctx.as_mut_ptr() }, None);
    let mut opts = ffmpeg::Dictionary::new();
    opts.set("preset", "veryfast");
    opts.set("crf", "20");
    // No B-frames: nothing is displayed ahead of a keyframe, so the preview
    // does not need tail patches for the proxy itself.
    opts.set(
        "x264-params",
        "bframes=0:keyint=60:min-keyint=60:scenecut=0",
    );
    let mut enc = ctx
        .open_as_with(codec, opts)
        .ctx(|| "opening libx264".into())?;
    let mut ost = octx
        .add_stream(Some(codec))
        .ctx(|| "adding the proxy stream".into())?;
    ost.set_parameters(&enc);
    ost.set_time_base(enc_tb);
    octx.write_header()
        .ctx(|| "writing the proxy header".into())?;

    let mut normalizer: Option<Normalizer> = None;
    let mut durations: HashMap<i64, i64> = HashMap::new();
    let mut pending: Option<(frame::Video, i64)> = None;
    let mut last_dur = 1i64;
    let mut frames = 0u64;
    let encode_one = |enc: &mut encoder::Video,
                      octx: &mut format::context::Output,
                      durations: &mut HashMap<i64, i64>,
                      mut picture: frame::Video,
                      pts: i64,
                      dur: i64|
     -> Result<()> {
        picture.set_pts(Some(pts));
        unsafe { (*picture.as_mut_ptr()).duration = dur };
        durations.insert(pts, dur);
        enc.send_frame(&picture)
            .ctx(|| "encoding a proxy frame".into())?;
        drain(enc, octx, enc_tb, durations)
    };
    while let Some(src) = reader.next_frame()? {
        let Some(raw) = src.pts() else {
            return Err(Error::Invalid(
                "a source frame has no timestamp to copy onto the proxy".into(),
            ));
        };
        let mut pts = scale_pts(raw, src_tb, enc_tb);
        if let Some((_, prev)) = &pending
            && pts <= *prev
        {
            pts = prev.saturating_add(1);
        }
        let picture = scale(&mut normalizer, &src, rotation, width, height)?;
        if let Some((prev, prev_pts)) = pending.take() {
            let dur = (pts - prev_pts).max(1);
            last_dur = dur;
            encode_one(&mut enc, &mut octx, &mut durations, prev, prev_pts, dur)?;
            frames += 1;
        }
        pending = Some((picture, pts));
    }
    if let Some((prev, prev_pts)) = pending.take() {
        encode_one(
            &mut enc,
            &mut octx,
            &mut durations,
            prev,
            prev_pts,
            last_dur,
        )?;
        frames += 1;
    }
    if frames == 0 {
        return Err(Error::Invalid(format!(
            "{}: no pictures to proxy",
            reader.path().display()
        )));
    }
    enc.send_eof().ctx(|| "flushing the proxy encoder".into())?;
    drain(&mut enc, &mut octx, enc_tb, &mut durations)?;
    octx.write_trailer().ctx(|| "finishing the proxy".into())?;
    Ok(frames)
}

fn scale(
    normalizer: &mut Option<Normalizer>,
    src: &frame::Video,
    rotation: u16,
    width: u32,
    height: u32,
) -> Result<frame::Video> {
    if normalizer
        .as_ref()
        .is_none_or(|n| !n.matches(src, rotation))
    {
        *normalizer = Some(Normalizer::new(
            src,
            rotation,
            width,
            height,
            format::Pixel::YUV420P,
            None,
        )?);
    }
    normalizer.as_mut().expect("just built").run(src)
}

fn drain(
    enc: &mut encoder::Video,
    octx: &mut format::context::Output,
    enc_tb: ffmpeg::Rational,
    durations: &mut HashMap<i64, i64>,
) -> Result<()> {
    let stream_tb = octx.stream(0).expect("proxy stream").time_base();
    let mut packet = ffmpeg::Packet::empty();
    while enc.receive_packet(&mut packet).is_ok() {
        if let Some(d) = packet.pts().and_then(|pts| durations.remove(&pts)) {
            packet.set_duration(d);
        }
        packet.set_stream(0);
        packet.rescale_ts(enc_tb, stream_tb);
        packet
            .write_interleaved(octx)
            .ctx(|| "writing a proxy packet".into())?;
    }
    Ok(())
}
