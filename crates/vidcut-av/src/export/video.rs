//! The video of a render plan: frames picked by the sampling rule and
//! normalized to the output size and pixel format.
//!
//! Sampling rule (docs/decisions/export.md): output frame `k` of a source piece
//! shows the first source frame whose timestamp is at least
//! `origin + (source_frame + k) / fps − ½ frame`. That is frame-exact for
//! sources at the project rate and nearest-frame sampling otherwise.

use std::path::Path;

use vidcut_core::render::VideoPiece;
use vidcut_core::time::Rational;

use crate::decode::VideoReader;
use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, filter, format, frame};

/// A new reference to `src`'s buffers (no pixel copy).
pub(crate) fn frame_ref(src: &frame::Video) -> frame::Video {
    let mut dst = frame::Video::empty();
    unsafe { ffmpeg::ffi::av_frame_ref(dst.as_mut_ptr(), src.as_ptr()) };
    dst
}

/// Scales (keeping the display aspect, honoring the sample aspect ratio),
/// pads to the output size, and converts to the output pixel format.
struct Normalizer {
    graph: filter::Graph,
    /// Input parameters the graph was built for.
    input: (u32, u32, format::Pixel, (i32, i32)),
}

/// The scaled size of a `w`×`h` picture with sample aspect `sar` fitted inside
/// `out_w`×`out_h`, rounded to even numbers.
pub(crate) fn fit(w: u32, h: u32, sar: (i32, i32), out_w: u32, out_h: u32) -> (u32, u32) {
    let sar = if sar.0 > 0 && sar.1 > 0 {
        f64::from(sar.0) / f64::from(sar.1)
    } else {
        1.0
    };
    let dar = f64::from(w) * sar / f64::from(h);
    let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
    if dar > f64::from(out_w) / f64::from(out_h) {
        (out_w, even(f64::from(out_w) / dar).min(out_h))
    } else {
        (even(f64::from(out_h) * dar).min(out_w), out_h)
    }
}

impl Normalizer {
    fn new(src: &frame::Video, out_w: u32, out_h: u32, out_fmt: format::Pixel) -> Result<Self> {
        let (w, h, fmt) = (src.width(), src.height(), src.format());
        let sar = src.aspect_ratio();
        let sar = (sar.numerator(), sar.denominator());
        let (sw, sh) = fit(w, h, sar, out_w, out_h);
        let fmt_name = |p: format::Pixel| {
            p.descriptor()
                .map(|d| d.name().to_string())
                .unwrap_or_default()
        };
        let mut graph = filter::Graph::new();
        let args = format!(
            "video_size={w}x{h}:pix_fmt={}:time_base=1/1000000:pixel_aspect={}/{}",
            ffmpeg::ffi::AVPixelFormat::from(fmt) as i32,
            sar.0.max(1),
            sar.1.max(1)
        );
        graph
            .add(
                &filter::find("buffer").ok_or_else(|| Error::Invalid("no buffer filter".into()))?,
                "in",
                &args,
            )
            .ctx(|| "creating video source".into())?;
        graph
            .add(
                &filter::find("buffersink")
                    .ok_or_else(|| Error::Invalid("no buffersink filter".into()))?,
                "out",
                "",
            )
            .ctx(|| "creating video sink".into())?;
        let spec = format!(
            "scale={sw}:{sh}:flags=bicubic,pad={out_w}:{out_h}:{}:{}:color=black,setsar=1,format={}",
            (out_w - sw) / 2,
            (out_h - sh) / 2,
            fmt_name(out_fmt)
        );
        graph
            .output("in", 0)
            .and_then(|p| p.input("out", 0))
            .and_then(|p| p.parse(&spec))
            .ctx(|| format!("building filter {spec}"))?;
        graph
            .validate()
            .ctx(|| format!("configuring filter {spec}"))?;
        Ok(Self {
            graph,
            input: (w, h, fmt, sar),
        })
    }

    fn matches(&self, src: &frame::Video) -> bool {
        let sar = src.aspect_ratio();
        self.input
            == (
                src.width(),
                src.height(),
                src.format(),
                (sar.numerator(), sar.denominator()),
            )
    }

    fn run(&mut self, src: &frame::Video) -> Result<frame::Video> {
        let mut input = frame_ref(src);
        input.set_pts(Some(0));
        self.graph
            .get("in")
            .expect("source")
            .source()
            .add(&input)
            .ctx(|| "filtering frame".into())?;
        let mut out = frame::Video::empty();
        self.graph
            .get("out")
            .expect("sink")
            .sink()
            .frame(&mut out)
            .ctx(|| "filtering frame".into())?;
        Ok(out)
    }
}

/// An open source piece.
struct Source {
    reader: VideoReader,
    /// Seconds per tick of the reader's time base.
    tick: f64,
    origin: f64,
    source_frame: i64,
    /// Current source frame (decoded) and its timestamp in seconds.
    current: Option<(frame::Video, f64)>,
    /// `current`, normalized.
    normalized: Option<frame::Video>,
}

/// Produces the plan's video frames in order.
pub struct VideoFrames {
    pieces: Vec<VideoPiece>,
    fps: Rational,
    out: (u32, u32, format::Pixel),
    piece: usize,
    /// Frame index within the current piece.
    k: i64,
    source: Option<Source>,
    normalizer: Option<Normalizer>,
    black: Option<frame::Video>,
}

impl VideoFrames {
    pub fn new(
        pieces: &[VideoPiece],
        fps: Rational,
        width: u32,
        height: u32,
        pixel: format::Pixel,
    ) -> Self {
        Self {
            pieces: pieces.to_vec(),
            fps,
            out: (width, height, pixel),
            piece: 0,
            k: 0,
            source: None,
            normalizer: None,
            black: None,
        }
    }

    /// The next output frame (pts unset), or `None` after the last one.
    pub fn next_frame(&mut self) -> Result<Option<frame::Video>> {
        loop {
            let Some(piece) = self.pieces.get(self.piece) else {
                return Ok(None);
            };
            if self.k >= piece.frames() {
                self.piece += 1;
                self.k = 0;
                self.source = None;
                continue;
            }
            let k = self.k;
            self.k += 1;
            return match piece.clone() {
                VideoPiece::Black { .. } => Ok(Some(frame_ref(self.black()?))),
                VideoPiece::Source {
                    path,
                    origin,
                    source_frame,
                    ..
                } => {
                    if self.source.is_none() {
                        self.source = Some(self.open(&path, origin, source_frame)?);
                    }
                    self.source_frame(k).map(Some)
                }
            };
        }
    }

    fn black(&mut self) -> Result<&frame::Video> {
        if self.black.is_none() {
            let (w, h, pix) = self.out;
            let mut f = frame::Video::new(pix, w, h);
            fill_black(&mut f)?;
            self.black = Some(f);
        }
        Ok(self.black.as_ref().expect("just set"))
    }

    /// Seconds of the sampling-rule target for source position `pos`.
    fn target(&self, origin: f64, pos: i64) -> f64 {
        origin + self.fps.frames_to_secs(pos) - 0.5 * self.fps.frames_to_secs(1)
    }

    fn open(&self, path: &Path, origin: f64, source_frame: i64) -> Result<Source> {
        let mut reader = VideoReader::open(path)?;
        let tb = reader.time_base();
        let tick = f64::from(tb.numerator()) / f64::from(tb.denominator());
        let target = self.target(origin, source_frame);
        reader.seek((target / tick).floor() as i64)?;
        Ok(Source {
            reader,
            tick,
            origin,
            source_frame,
            current: None,
            normalized: None,
        })
    }

    /// Output frame `k` of the current source piece.
    fn source_frame(&mut self, k: i64) -> Result<frame::Video> {
        let (out_w, out_h, out_fmt) = self.out;
        let fps = self.fps;
        let src = self.source.as_mut().expect("open");
        let target =
            src.origin + fps.frames_to_secs(src.source_frame + k) - 0.5 * fps.frames_to_secs(1);
        // Advance to the first frame at or after the target (1 µs of slack for
        // float noise); past the end of the stream, keep the last frame.
        let mut changed = false;
        while src.current.as_ref().is_none_or(|(_, t)| *t < target - 1e-6) {
            match src.reader.next_frame()? {
                Some(f) => {
                    let t = f.pts().unwrap_or(0) as f64 * src.tick;
                    src.current = Some((f, t));
                    changed = true;
                }
                None if src.current.is_some() => break,
                None => {
                    return Err(Error::Invalid(format!(
                        "source ends before frame {}",
                        src.source_frame + k
                    )));
                }
            }
        }
        if changed || src.normalized.is_none() {
            let (frame, _) = src.current.as_ref().expect("have a frame");
            let rebuild = self.normalizer.as_ref().is_none_or(|n| !n.matches(frame));
            if rebuild {
                self.normalizer = Some(Normalizer::new(frame, out_w, out_h, out_fmt)?);
            }
            src.normalized = Some(self.normalizer.as_mut().expect("built").run(frame)?);
        }
        Ok(frame_ref(src.normalized.as_ref().expect("normalized")))
    }
}

/// Fills a frame with black: video-range for YUV, zero for RGB, opaque alpha.
pub(crate) fn fill_black(f: &mut frame::Video) -> Result<()> {
    let desc = f
        .format()
        .descriptor()
        .ok_or_else(|| Error::Invalid("unknown pixel format".into()))?;
    let (deep, rgb, alpha) = unsafe {
        let d = &*desc.as_ptr();
        let flags = d.flags;
        (
            d.comp[0].depth > 8,
            flags & (ffmpeg::ffi::AV_PIX_FMT_FLAG_RGB as u64) != 0,
            flags & (ffmpeg::ffi::AV_PIX_FMT_FLAG_ALPHA as u64) != 0,
        )
    };
    let planes = f.planes();
    for plane in 0..planes {
        let is_alpha = alpha && plane == planes - 1 && planes > 1;
        let v8: u16 = if is_alpha {
            255
        } else if rgb {
            0
        } else if plane == 0 {
            16
        } else {
            128
        };
        let data = f.data_mut(plane);
        if deep {
            let v = if is_alpha { 1023 } else { v8 << 2 };
            for px in data.chunks_exact_mut(2) {
                px.copy_from_slice(&v.to_le_bytes());
            }
        } else {
            data.fill(v8 as u8);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::fit;

    #[test]
    fn fitting_keeps_display_aspect() {
        assert_eq!(fit(1920, 1080, (1, 1), 1920, 1080), (1920, 1080));
        assert_eq!(fit(1280, 720, (1, 1), 1920, 1080), (1920, 1080));
        assert_eq!(
            fit(1440, 1080, (1, 1), 1920, 1080),
            (1440, 1080),
            "4:3 pillarboxed"
        );
        assert_eq!(
            fit(1440, 1080, (4, 3), 1920, 1080),
            (1920, 1080),
            "anamorphic 16:9"
        );
        assert_eq!(
            fit(1920, 1080, (1, 1), 1080, 1920),
            (1080, 608),
            "landscape into vertical"
        );
        assert_eq!(fit(720, 576, (16, 15), 1920, 1080), (1440, 1080), "PAL 4:3");
    }
}
