//! Stream-copy remuxing, used to give index-less sources a seek index.
//!
//! mpv cannot seek frame-exactly inside EDL segments of containers without a
//! seek index (MPEG-TS/PS, raw elementary streams): the demuxer lands after the
//! target and the segment is skipped. Preview therefore plays an MKV remux of
//! such sources (same packets, same timestamps), while export keeps reading the
//! original through vidcut's own decoder, which handles them.

use std::path::Path;

use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, codec, format, media};

/// Whether a demuxer (by its short name, as in
/// [`MediaInfo::format`](crate::probe::MediaInfo::format)) lacks a seek index,
/// so preview needs an [`to_mkv`] remux.
pub fn needs_index(format: &str) -> bool {
    format.split(',').any(|f| {
        matches!(
            f,
            "mpegts"
                | "mpegtsraw"
                | "mpeg"
                | "mpegvideo"
                | "h264"
                | "hevc"
                | "vvc"
                | "m4v"
                | "dvd"
                | "vob"
        )
    })
}

/// Copies the audio and video streams of `input` into a Matroska file at
/// `output`, keeping packets and timestamps unchanged (Matroska rounds them to
/// 1 ms, which the time map's half-frame bias absorbs).
pub fn to_mkv(input: &Path, output: &Path) -> Result<()> {
    crate::init();
    let mut ictx = format::input(input).ctx(|| format!("opening {}", input.display()))?;
    let mut octx =
        format::output_as(output, "matroska").ctx(|| format!("creating {}", output.display()))?;

    let mut map: Vec<Option<(usize, ffmpeg::Rational)>> = vec![None; ictx.nb_streams() as usize];
    for ist in ictx.streams() {
        let medium = ist.parameters().medium();
        if medium != media::Type::Video && medium != media::Type::Audio {
            continue;
        }
        let mut ost = octx
            .add_stream(ffmpeg::encoder::find(codec::Id::None))
            .ctx(|| "adding stream".into())?;
        ost.set_parameters(ist.parameters());
        // The source container's codec tag may be invalid in Matroska.
        unsafe { (*ost.parameters().as_mut_ptr()).codec_tag = 0 };
        map[ist.index()] = Some((ost.index(), ist.time_base()));
    }
    if map.iter().all(Option::is_none) {
        return Err(Error::Invalid(format!(
            "{}: nothing to remux",
            input.display()
        )));
    }
    octx.write_header().ctx(|| "writing header".into())?;
    for (stream, mut packet) in ictx.packets() {
        let Some((out_index, in_tb)) = map[stream.index()] else {
            continue;
        };
        let out_tb = octx.stream(out_index).expect("mapped stream").time_base();
        packet.rescale_ts(in_tb, out_tb);
        packet.set_position(-1);
        packet.set_stream(out_index);
        if packet.size() == 0 {
            continue;
        }
        packet
            .write_interleaved(&mut octx)
            .ctx(|| "writing packet".into())?;
    }
    octx.write_trailer().ctx(|| "writing trailer".into())?;
    Ok(())
}
