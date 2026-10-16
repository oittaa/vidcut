//! Single frames as pictures, as export samples them: the timeline's
//! filmstrip tiles, storyboards and `frame render`; and the window rule the
//! player's decode thread goes back by (docs/decisions/player.md §3).

use vidcut_core::render::VideoPiece;
use vidcut_core::time::Rational;

use crate::error::{Error, Result};
use crate::ffmpeg::{format, frame};
use crate::thumbs::Image;

use super::video::VideoFrames;

/// Single frames as small pictures, in the order they are asked for: a
/// timeline's filmstrip tiles. Nothing is kept. The file stays open, so a
/// frame shortly after the one before decodes on from it instead of seeking;
/// ask for a file's frames in order.
#[derive(Default)]
pub struct FrameReader {
    frames: Option<(Rational, u32, u32, VideoFrames)>,
    /// Where files decode, when on a GPU ([`decoding_on`](Self::decoding_on)).
    decoder: Option<crate::hw::GpuDecoder>,
}

impl FrameReader {
    /// Files decode on `gpu` where it decodes their streams, the frames
    /// shrunk there before they come down ([`VideoFrames::decode_on`]).
    pub fn decoding_on(gpu: crate::hw::GpuDecoder) -> Self {
        Self {
            frames: None,
            decoder: Some(gpu),
        }
    }

    /// The GPU that decoded the last frame it rendered (`None`: the CPU,
    /// or black).
    pub fn decoded_on(&self) -> Option<crate::hw::Kind> {
        self.frames.as_ref().and_then(|(.., f)| f.decoded_on())
    }

    /// `piece`'s first frame as export samples it, at `width`×`height`
    /// (packed RGBA, the picture fit inside, padded black).
    pub fn rgba(
        &mut self,
        piece: &VideoPiece,
        fps: Rational,
        width: u32,
        height: u32,
    ) -> Result<Image> {
        crate::cancel::stop()?;
        crate::init();
        let same =
            matches!(&self.frames, Some((f, w, h, _)) if (*f, *w, *h) == (fps, width, height));
        if !same {
            let frames = frames(fps, (width, height), RGBA, self.decoder.as_ref());
            self.frames = Some((fps, width, height, frames));
        }
        let (.., frames) = self.frames.as_mut().expect("set above");
        frames.read_piece(&piece.part(0, 1));
        let frame = frames
            .next_frame()?
            .ok_or_else(|| Error::Invalid("no frame to render".into()))?;
        Ok(image(&frame, width, height))
    }
}

/// Pictures packed for these renderers' images.
const RGBA: format::Pixel = format::Pixel::RGBA;

/// Frames as `pixel` pictures of `width`×`height` at `fps`, decoding on
/// `decoder` where it decodes a stream.
pub(crate) fn frames(
    fps: Rational,
    (width, height): (u32, u32),
    pixel: format::Pixel,
    decoder: Option<&crate::hw::GpuDecoder>,
) -> VideoFrames {
    let frames = VideoFrames::new(&[], fps, width, height, pixel, None);
    match decoder {
        Some(d) => frames.decode_on(d.clone()),
        None => frames,
    }
}

/// `over` at `weight` over `under`, packed RGBA pictures of one size
/// ([`super::mix::mixed`]).
pub(crate) fn mixed_image(under: &Image, over: &Image, weight: u32) -> Image {
    let rgba = under
        .rgba
        .iter()
        .zip(&over.rgba)
        .map(|(&a, &b)| super::mix::mixed(u32::from(a), u32::from(b), weight) as u8)
        .collect();
    Image {
        width: under.width,
        height: under.height,
        rgba,
    }
}

/// The first frame of the window to render together for `pos`, going back:
/// from a keyframe, so nothing in it decodes twice, reaching back over more
/// keyframes to hold `min` frames, holding at most `max`, and starting no
/// earlier than `floor`. With `whole_gops` it reaches back over another
/// keyframe only to hold that GOP whole: started between two keyframes, the
/// frames before it decode for nothing.
#[allow(clippy::too_many_arguments)]
pub(crate) fn window(
    frames: &mut VideoFrames,
    piece: &VideoPiece,
    pos: i64,
    floor: i64,
    fps: Rational,
    min: i64,
    max: i64,
    whole_gops: bool,
) -> Result<i64> {
    let VideoPiece::Source { origin, .. } = piece else {
        return Ok(pos);
    };
    let rate = fps.as_f64();
    let (mut probe, mut start) = (pos, pos);
    for i in 0..64 {
        let Some(key) = frames.keyframe_time(&at(piece, probe, 1))? else {
            start = pos - min + 1;
            break;
        };
        // The first frame sampled at or after the keyframe: its target time
        // is past the keyframe's (a microsecond more against float noise),
        // so seeking for it starts from that keyframe, not the one before.
        let first = ((key - origin) * rate + 0.5 + 1e-6).ceil() as i64;
        let first = first.clamp(0, probe);
        if whole_gops && i > 0 && pos - first.max(floor) + 1 > max {
            break;
        }
        start = first;
        if pos - start + 1 >= min || start <= floor {
            break;
        }
        probe = start - 1;
    }
    Ok(start.max(pos - max + 1).max(floor).max(0))
}

/// `piece` from source position `pos`, `frames` long, walking its source
/// frame by frame; a mix's first `frames`.
pub(crate) fn at(piece: &VideoPiece, pos: i64, frames: i64) -> VideoPiece {
    let mut p = piece.clone();
    match &mut p {
        VideoPiece::Mix { .. } => return piece.part(0, frames),
        VideoPiece::Source {
            source_frame,
            frames: n,
            pace,
            ..
        } => {
            *source_frame = pos;
            *n = frames;
            *pace = Default::default();
        }
        VideoPiece::Black { frames: n } => *n = frames,
    }
    p
}

/// A packed RGBA frame's rows as a picture.
fn image(frame: &frame::Video, width: u32, height: u32) -> Image {
    let stride = frame.stride(0);
    let row = width as usize * 4;
    let data = frame.data(0);
    let mut rgba = Vec::with_capacity(row * height as usize);
    for y in 0..height as usize {
        rgba.extend_from_slice(&data[y * stride..y * stride + row]);
    }
    Image {
        width,
        height,
        rgba,
    }
}
