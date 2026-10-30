use vidcut_core::model::Hdr;
use vidcut_core::render::VideoPiece;
use vidcut_core::time::Rational;

use crate::error::{Error, Result};
use crate::ffmpeg::format;
use crate::thumbs::Image;

use super::video::VideoFrames;

type Opened = (Rational, u32, u32, Option<Hdr>, VideoFrames);

#[derive(Default)]
pub struct FrameReader {
    frames: Option<Opened>,
    decoder: Option<crate::hw::GpuDecoder>,
}

impl FrameReader {
    pub fn decoding_on(gpu: crate::hw::GpuDecoder) -> Self {
        Self {
            frames: None,
            decoder: Some(gpu),
        }
    }

    pub fn decoded_on(&self) -> Option<crate::hw::Kind> {
        self.frames.as_ref().and_then(|(.., f)| f.decoded_on())
    }

    pub fn rgba(
        &mut self,
        piece: &VideoPiece,
        fps: Rational,
        (width, height): (u32, u32),
        colors: Option<Hdr>,
    ) -> Result<Image> {
        crate::cancel::stop()?;
        crate::init();
        let same = matches!(&self.frames, Some((f, w, h, c, _))
            if (*f, *w, *h, *c) == (fps, width, height, colors));
        if !same {
            let frames = frames(fps, (width, height), RGBA, colors, self.decoder.as_ref())?;
            self.frames = Some((fps, width, height, colors, frames));
        }
        let (.., frames) = self.frames.as_mut().expect("set above");
        frames.read_piece(&piece.part(0, 1));
        let frame = frames
            .next_frame()?
            .ok_or_else(|| Error::Invalid("no frame to render".into()))?;
        Ok(Image::of(&frame))
    }
}

const RGBA: format::Pixel = format::Pixel::RGBA;

pub(crate) fn frames(
    fps: Rational,
    (width, height): (u32, u32),
    pixel: format::Pixel,
    colors: Option<Hdr>,
    decoder: Option<&crate::hw::GpuDecoder>,
) -> Result<VideoFrames> {
    let frames = VideoFrames::new(&[], fps, width, height, pixel, None)
        .single_frames()
        .with_timeline(colors)?;
    Ok(match decoder {
        Some(d) => frames.decode_on(d.clone()),
        None => frames,
    })
}

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

pub(crate) fn at(piece: &VideoPiece, pos: i64, frames: i64) -> VideoPiece {
    let mut p = piece.clone();
    match &mut p {
        VideoPiece::Mix { .. } | VideoPiece::Titled { .. } => return piece.part(0, frames),
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
        VideoPiece::Black { frames: n, .. } => *n = frames,
    }
    p
}
