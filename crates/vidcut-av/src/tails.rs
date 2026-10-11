//! Frames mpv's EDL playback would lose at the end of a cut, and the small
//! clips that stand in for them (docs/decisions/preview.md §9).
//!
//! In many encodes (open GOPs, and x264's B-frames before a keyframe) the
//! keyframe after a cut is decoded *before* the frames displayed just ahead of
//! it ("leading pictures"). mpv ends an EDL segment at the first keyframe past
//! the segment's end, so those last frames of the segment never reach the
//! decoder and the viewer shows the next segment early. Export is unaffected:
//! it selects frames by timestamp after decoding.
//!
//! The preview patches such a tail with a few frames rendered by the export
//! engine itself (so preview equals export) into a lossless all-intra file.

use std::path::Path;

use vidcut_core::render::VideoPiece;
use vidcut_core::time::Rational;

use crate::error::{Context, Error, Result};
use crate::export::video::VideoFrames;
use crate::ffmpeg::{self, codec, encoder, format, media};

/// When a segment of `path`'s video ends at `end` (raw stream seconds, as in
/// an EDL), the earliest timestamp of the frames before `end` that mpv would
/// drop, or `None` if it drops none.
pub fn lost_before(path: &Path, end: f64) -> Result<Option<f64>> {
    crate::init();
    let mut input = format::input(path).ctx(|| format!("opening {}", path.display()))?;
    let stream = input
        .streams()
        .best(media::Type::Video)
        .ok_or_else(|| Error::Invalid(format!("{}: no video stream", path.display())))?;
    let (index, tb) = (stream.index(), f64::from(stream.time_base()));
    crate::export::audio::discard_others(&input, index);
    // From the keyframe before the cut: what matters is the first keyframe
    // past it and the frames decoded right after that keyframe.
    let ts = (end.max(0.0) * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
    let _ = input.seek(ts, ..ts);
    let mut keyframe = false;
    let mut after = 0;
    let mut lost: Option<f64> = None;
    for (s, packet) in input.packets() {
        if s.index() != index {
            continue;
        }
        let Some(pts) = packet.pts().map(|t| t as f64 * tb) else {
            continue;
        };
        if !keyframe {
            if packet.is_key() && pts >= end {
                keyframe = true;
            } else if pts > end + 60.0 {
                break; // no keyframe anywhere near: nothing to lose
            }
            continue;
        }
        if pts < end {
            lost = Some(lost.map_or(pts, |l: f64| l.min(pts)));
        }
        // Leading pictures follow their keyframe closely in decode order.
        after += 1;
        if after >= 32 {
            break;
        }
    }
    Ok(lost)
}

/// Renders `piece` (normally a few frames) exactly as export would, at
/// `width`×`height` and `fps`, into a lossless all-intra Matroska file whose
/// first frame has timestamp 0.
pub fn render_patch(
    piece: &VideoPiece,
    fps: Rational,
    width: u32,
    height: u32,
    out: &Path,
) -> Result<()> {
    crate::init();
    let pixel = format::Pixel::YUV420P;
    let mut frames = VideoFrames::new(std::slice::from_ref(piece), fps, width, height, pixel);
    let rate = ffmpeg::Rational(fps.num() as i32, fps.den() as i32);
    let tmp = out.with_extension("part.mkv");
    let result = (|| -> Result<()> {
        let mut octx =
            format::output_as(&tmp, "matroska").ctx(|| format!("creating {}", tmp.display()))?;
        let codec = encoder::find_by_name("ffv1")
            .ok_or_else(|| Error::Invalid("this FFmpeg has no FFV1 encoder".into()))?;
        let mut ctx = codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()
            .ctx(|| "creating FFV1 encoder".into())?;
        ctx.set_width(width);
        ctx.set_height(height);
        ctx.set_format(pixel);
        ctx.set_time_base(rate.invert());
        ctx.set_frame_rate(Some(rate));
        ctx.set_gop(1);
        if octx.format().flags().contains(format::Flags::GLOBAL_HEADER) {
            ctx.set_flags(codec::Flags::GLOBAL_HEADER);
        }
        let mut opts = ffmpeg::Dictionary::new();
        opts.set("level", "3");
        opts.set("slices", "4");
        let mut enc = ctx
            .open_as_with(codec, opts)
            .ctx(|| "opening FFV1 encoder".into())?;
        let mut ost = octx
            .add_stream(Some(codec))
            .ctx(|| "adding video stream".into())?;
        ost.set_parameters(&enc);
        ost.set_time_base(rate.invert());
        ost.set_avg_frame_rate(rate);
        octx.write_header().ctx(|| "writing header".into())?;
        let stream_tb = octx.stream(0).expect("added").time_base();
        let drain = |enc: &mut encoder::Video, octx: &mut format::context::Output| {
            let mut p = ffmpeg::Packet::empty();
            while enc.receive_packet(&mut p).is_ok() {
                p.set_stream(0);
                p.set_duration(1);
                p.rescale_ts(rate.invert(), stream_tb);
                crate::export::write_packet(octx, &p)?;
            }
            Ok::<(), Error>(())
        };
        let mut k = 0;
        while let Some(mut f) = frames.next_frame()? {
            f.set_pts(Some(k));
            k += 1;
            enc.send_frame(&f).ctx(|| "encoding a patch frame".into())?;
            drain(&mut enc, &mut octx)?;
        }
        enc.send_eof().ctx(|| "flushing FFV1".into())?;
        drain(&mut enc, &mut octx)?;
        octx.write_trailer().ctx(|| "writing trailer".into())?;
        Ok(())
    })();
    match result {
        Ok(()) => Ok(std::fs::rename(&tmp, out)?),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}
