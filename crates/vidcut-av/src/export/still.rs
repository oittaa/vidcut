//! Still frames as PNG: what the timeline (or a media file) shows at a frame,
//! picked by the same sampling rule as export. Lets agents look at a cut.

use std::collections::HashMap;
use std::path::Path;

use vidcut_core::render::VideoPiece;
use vidcut_core::time::Rational;

use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, codec, encoder, format};

use super::video::VideoFrames;

/// Renders one video piece frame (a single-frame piece) as PNG bytes at
/// `width`×`height` (the picture fit inside, padded black).
pub fn png(piece: &VideoPiece, fps: Rational, width: u32, height: u32) -> Result<Vec<u8>> {
    crate::cancel::stop()?;
    crate::init();
    let mut frames = VideoFrames::new(
        std::slice::from_ref(piece),
        fps,
        width,
        height,
        format::Pixel::RGB24,
        None,
    );
    let mut frame = frames
        .next_frame()?
        .ok_or_else(|| Error::Invalid("no frame to render".into()))?;
    frame.set_pts(Some(0));
    let codec = encoder::find_by_name("png")
        .ok_or_else(|| Error::Invalid("this FFmpeg has no PNG encoder".into()))?;
    let mut ctx = codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()
        .ctx(|| "creating PNG encoder".into())?;
    ctx.set_width(width);
    ctx.set_height(height);
    ctx.set_format(format::Pixel::RGB24);
    ctx.set_time_base(ffmpeg::Rational(1, 25));
    let mut enc = ctx.open_as(codec).ctx(|| "opening PNG encoder".into())?;
    enc.send_frame(&frame).ctx(|| "encoding PNG".into())?;
    enc.send_eof().ctx(|| "encoding PNG".into())?;
    let mut packet = ffmpeg::Packet::empty();
    enc.receive_packet(&mut packet)
        .ctx(|| "encoding PNG".into())?;
    Ok(packet.data().unwrap_or_default().to_vec())
}

/// Renders one video piece frame (a single-frame piece) as packed RGBA at
/// `width`×`height` (the picture fit inside, padded black).
pub fn rgba(
    piece: &VideoPiece,
    fps: Rational,
    width: u32,
    height: u32,
) -> Result<crate::thumbs::Image> {
    StillRenderer::default().rgba(piece, fps, width, height)
}

/// Renders frames one after another as packed RGBA, keeping the file and
/// its decoder open between them: a frame shortly after the one before
/// decodes on from it, where [`rgba`] would seek and decode from a keyframe
/// again. For pictures that follow a moving position, such as a trim's.
///
/// Going back, a seek and the decoding from a keyframe are unavoidable, so
/// that pass renders the frames just before the one asked for too
/// ([`BEHIND`]) and keeps them: the next frames back are ready.
#[derive(Default)]
pub struct StillRenderer {
    /// The frames reader for one rate and size.
    frames: Option<(Rational, u32, u32, VideoFrames)>,
    /// The source and position of the frame rendered last.
    last: Option<(Source, i64)>,
    /// Frames rendered on the way back, by position, of `last`'s source.
    behind: HashMap<i64, crate::thumbs::Image>,
}

/// How many frames before a frame asked for going back are rendered with
/// it and kept.
pub const BEHIND: i64 = 60;

/// What a piece shows, apart from where in it: positions of one source
/// can stand in for each other.
#[derive(Debug, Clone, PartialEq)]
struct Source {
    path: std::path::PathBuf,
    origin: f64,
    image: Option<vidcut_core::render::Image>,
    fps: Rational,
    width: u32,
    height: u32,
}

impl StillRenderer {
    /// Renders `piece`'s first frame at `width`×`height` (the picture fit
    /// inside, padded black).
    pub fn rgba(
        &mut self,
        piece: &VideoPiece,
        fps: Rational,
        width: u32,
        height: u32,
    ) -> Result<crate::thumbs::Image> {
        crate::cancel::stop()?;
        crate::init();
        let same =
            matches!(&self.frames, Some((f, w, h, _)) if (*f, *w, *h) == (fps, width, height));
        if !same {
            let frames = VideoFrames::new(&[], fps, width, height, format::Pixel::RGBA, None);
            self.frames = Some((fps, width, height, frames));
        }
        let at = match piece {
            VideoPiece::Source {
                path,
                origin,
                source_frame,
                image,
                ..
            } => Some((
                Source {
                    path: path.clone(),
                    origin: *origin,
                    image: image.clone(),
                    fps,
                    width,
                    height,
                },
                *source_frame,
            )),
            VideoPiece::Black { .. } => None,
        };
        let previous = self.last.take();
        let Some((source, pos)) = at else {
            self.behind.clear();
            return self.render(piece, 1).map(|mut v| v.remove(0));
        };
        if previous.as_ref().is_none_or(|(s, _)| *s != source) {
            self.behind.clear();
        }
        let back = previous
            .as_ref()
            .is_some_and(|(s, p)| *s == source && pos < *p);
        self.last = Some((source, pos));
        if let Some(img) = self.behind.get(&pos) {
            return Ok(img.clone());
        }
        // Only a video file decodes from keyframes: a still's one picture
        // stands for every position, and a sequence's pictures each stand
        // alone.
        let video = matches!(piece, VideoPiece::Source { image: None, .. });
        if !back || !video {
            return self.render(piece, 1).map(|mut v| v.remove(0));
        }
        let from = (pos - (BEHIND - 1)).max(0);
        let mut range = piece.clone();
        if let VideoPiece::Source {
            source_frame,
            frames,
            ..
        } = &mut range
        {
            *source_frame = from;
            *frames = pos - from + 1;
        }
        let images = self.render(&range, pos - from + 1)?;
        // Keep these and the frames nearest them: a trim going back reads
        // on downwards.
        self.behind.retain(|&p, _| (pos - p).abs() < 2 * BEHIND);
        for (p, img) in (from..=pos).zip(images) {
            self.behind.insert(p, img);
        }
        Ok(self.behind[&pos].clone())
    }

    /// The first `n` frames of `piece`, in order.
    fn render(&mut self, piece: &VideoPiece, n: i64) -> Result<Vec<crate::thumbs::Image>> {
        let (_, width, height, frames) = self.frames.as_mut().expect("set by rgba");
        let (width, height) = (*width, *height);
        frames.read_piece(piece);
        let mut out = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let frame = frames
                .next_frame()?
                .ok_or_else(|| Error::Invalid("no frame to render".into()))?;
            let stride = frame.stride(0);
            let row = width as usize * 4;
            let data = frame.data(0);
            let mut rgba = Vec::with_capacity(row * height as usize);
            for y in 0..height as usize {
                rgba.extend_from_slice(&data[y * stride..y * stride + row]);
            }
            out.push(crate::thumbs::Image {
                width,
                height,
                rgba,
            });
        }
        Ok(out)
    }
}

/// Encodes a packed RGBA picture (rows top to bottom) as PNG.
pub fn png_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    crate::init();
    if rgba.len() != width as usize * height as usize * 4 || width == 0 || height == 0 {
        return Err(Error::Invalid(format!(
            "{width}x{height} RGBA needs {} bytes, got {}",
            width as usize * height as usize * 4,
            rgba.len()
        )));
    }
    let mut frame = ffmpeg::frame::Video::new(format::Pixel::RGBA, width, height);
    let stride = frame.stride(0);
    let row = width as usize * 4;
    for (y, src) in rgba.chunks_exact(row).enumerate() {
        frame.data_mut(0)[y * stride..y * stride + row].copy_from_slice(src);
    }
    frame.set_pts(Some(0));
    let codec = encoder::find_by_name("png")
        .ok_or_else(|| Error::Invalid("this FFmpeg has no PNG encoder".into()))?;
    let mut ctx = codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()
        .ctx(|| "creating PNG encoder".into())?;
    ctx.set_width(width);
    ctx.set_height(height);
    ctx.set_format(format::Pixel::RGBA);
    ctx.set_time_base(ffmpeg::Rational(1, 25));
    let mut enc = ctx.open_as(codec).ctx(|| "opening PNG encoder".into())?;
    enc.send_frame(&frame).ctx(|| "encoding PNG".into())?;
    enc.send_eof().ctx(|| "encoding PNG".into())?;
    let mut packet = ffmpeg::Packet::empty();
    enc.receive_packet(&mut packet)
        .ctx(|| "encoding PNG".into())?;
    Ok(packet.data().unwrap_or_default().to_vec())
}

/// Writes a PNG of one piece frame to `path`.
pub fn write_png(
    piece: &VideoPiece,
    fps: Rational,
    width: u32,
    height: u32,
    path: &Path,
) -> Result<()> {
    let bytes = png(piece, fps, width, height)?;
    std::fs::write(path, bytes)?;
    Ok(())
}
