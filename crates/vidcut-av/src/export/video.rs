//! The video of a render plan: frames picked by the sampling rule and
//! normalized to the output size and pixel format.
//!
//! Sampling rule (docs/decisions/export.md): output frame `k` of a source piece
//! shows the first source frame whose timestamp is at least
//! `origin + (source_frame + k) / fps − ½ frame`. That is frame-exact for
//! sources at the project rate and nearest-frame sampling otherwise.

use std::path::Path;

use vidcut_core::model::Hdr;
use vidcut_core::render::{Image, VideoPiece};
use vidcut_core::time::Rational;

use super::ahead::Ahead;
use crate::decode::{Input, VideoReader};
use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, filter, format, frame};

/// How a piece's pictures are read: its file, or its image sequence.
pub(crate) fn input_of(path: &Path, image: Option<&Image>) -> Input {
    match image {
        Some(Image::Sequence {
            pattern,
            first,
            rate,
            ..
        }) => Input::Sequence {
            pattern: pattern.clone(),
            first: *first,
            rate: (rate.num() as i32, rate.den() as i32),
        },
        Some(Image::Still) | None => Input::File(path.to_path_buf()),
    }
}

/// What a piece's file holds: video, a still's one picture, or an image
/// sequence of `count` pictures.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pictures {
    Video,
    Still,
    Sequence { count: u64 },
}

impl Pictures {
    fn of(image: Option<&Image>) -> Self {
        match image {
            Some(Image::Still) => Self::Still,
            Some(Image::Sequence { count, .. }) => Self::Sequence { count: *count },
            None => Self::Video,
        }
    }
}

/// A new reference to `src`'s buffers (no pixel copy).
pub(crate) fn frame_ref(src: &frame::Video) -> Result<frame::Video> {
    let mut dst = frame::Video::empty();
    let ret = unsafe { ffmpeg::ffi::av_frame_ref(dst.as_mut_ptr(), src.as_ptr()) };
    if ret < 0 {
        return Err(Error::Ffmpeg {
            context: "referencing a frame".into(),
            source: ffmpeg::Error::from(ret),
        });
    }
    Ok(dst)
}

/// Turns pictures upright (the stream's rotation), scales them keeping the
/// display aspect (honoring the sample aspect ratio), pads to the output
/// size, and converts them to the output's colors and pixel format.
pub(crate) struct Normalizer {
    graph: filter::Graph,
    /// Input parameters the graph was built for, and the rotation.
    input: (
        u32,
        u32,
        format::Pixel,
        (i32, i32),
        u16,
        crate::color::Source,
    ),
}

/// The filters that turn a picture stored with clockwise `rotation` upright.
fn upright(rotation: u16) -> &'static str {
    match rotation {
        90 => "transpose=clock,",
        180 => "hflip,vflip,",
        270 => "transpose=cclock,",
        _ => "",
    }
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
    /// `target`: the output's dynamic range (`None`: SDR BT.709).
    pub(crate) fn new(
        src: &frame::Video,
        rotation: u16,
        out_w: u32,
        out_h: u32,
        out_fmt: format::Pixel,
        target: Option<Hdr>,
    ) -> Result<Self> {
        let (w, h, fmt) = (src.width(), src.height(), src.format());
        let sar = src.aspect_ratio();
        let sar = (sar.numerator(), sar.denominator());
        // Fitted as shown: a quarter turn swaps the sides and the pixel aspect.
        let (sw, sh) = if rotation % 180 == 90 {
            fit(h, w, (sar.1, sar.0), out_w, out_h)
        } else {
            fit(w, h, sar, out_w, out_h)
        };
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
        let color = crate::color::Source::of(src);
        let conv = crate::color::conversion(color, out_fmt, target);
        let spec = format!(
            "{}scale={sw}:{sh}:flags=bicubic{},pad={out_w}:{out_h}:{}:{}:color=black,setsar=1{},format={}",
            upright(rotation),
            conv.scale,
            (out_w - sw) / 2,
            (out_h - sh) / 2,
            conv.after,
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
            input: (w, h, fmt, sar, rotation, color),
        })
    }

    pub(crate) fn matches(&self, src: &frame::Video, rotation: u16) -> bool {
        let sar = src.aspect_ratio();
        self.input
            == (
                src.width(),
                src.height(),
                src.format(),
                (sar.numerator(), sar.denominator()),
                rotation,
                crate::color::Source::of(src),
            )
    }

    pub(crate) fn run(&mut self, src: &frame::Video) -> Result<frame::Video> {
        let mut input = frame_ref(src)?;
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

/// An open source file, read for one piece or several in a row.
struct Source {
    reader: VideoReader,
    /// Seconds per tick of the reader's time base.
    tick: f64,
    /// The current piece's origin and first source frame.
    origin: f64,
    source_frame: i64,
    /// The latest sampling target (seconds); every frame before `current` is
    /// earlier than it.
    target: f64,
    /// Current source frame (decoded) and its timestamp in seconds.
    current: Option<(frame::Video, f64)>,
    /// `current`, normalized.
    normalized: Option<frame::Video>,
    /// A still keeps its one picture and is never sought; a sequence knows
    /// how many pictures it has.
    pictures: Pictures,
    /// Decoded on NVDEC, and scaled on the GPU by `scaler` (built for the
    /// first frame), when the export's frames stay on the GPU.
    on_gpu: bool,
    scaler: Option<crate::cuda::CudaPictures>,
}

/// What a source needs to decode on NVDEC for an export whose frames stay
/// on the GPU: the device, and the output's size and format.
#[derive(Clone)]
pub(crate) struct GpuSide {
    device: crate::cuda::CudaDevice,
    out: (u32, u32),
    format: format::Pixel,
}

/// `input` opened on NVDEC, and the scaler to the output, when its pictures
/// go through the GPU as they would through the CPU: as for proxies
/// (upright, SDR, BT.709), progressive (frames are picked before any
/// scaling, so a deinterlacer would see gaps), and with filters the GPU
/// has for them (no `pad_cuda` for 10 bits).
fn gpu_reader(input: &Input, gpu: &GpuSide) -> Option<(VideoReader, crate::cuda::CudaPictures)> {
    let reader = VideoReader::open_cuda(input, &gpu.device).ok()?;
    let first = reader.peek()?;
    if !crate::proxy::gpu_suitable(first, reader.rotation()) || reader.interlaced() {
        return None;
    }
    let scaler =
        crate::cuda::CudaPictures::new(first, reader.time_base(), gpu.out, gpu.format, false)
            .ok()?;
    Some((reader, scaler))
}

/// How far past the current frame (seconds) a later piece of the same file
/// may start for decoding on to it to beat a seek.
const DECODE_AHEAD: f64 = 1.0;

impl Source {
    /// Opens `path` with `current` at the first frame for `target`. The
    /// decoding up to there is most of the cost of starting a piece, so the
    /// next file opens on another thread while this one encodes.
    ///
    /// A still's picture is already decoded by [`VideoReader::open_input`].
    /// Seeking would reopen the file and decode it again; a still is never
    /// sought.
    fn open(
        input: &Input,
        origin: f64,
        source_frame: i64,
        target: f64,
        pictures: Pictures,
        gpu: Option<&GpuSide>,
    ) -> Result<Source> {
        let on_gpu = gpu
            .filter(|_| pictures == Pictures::Video)
            .and_then(|g| gpu_reader(input, g));
        let (reader, scaler) = match on_gpu {
            Some((r, s)) => (r, Some(s)),
            None => (VideoReader::open_input(input)?, None),
        };
        let on_gpu = scaler.is_some();
        let tb = reader.time_base();
        let mut source = Source {
            reader,
            tick: f64::from(tb.numerator()) / f64::from(tb.denominator()),
            origin,
            source_frame,
            target,
            current: None,
            normalized: None,
            pictures,
            on_gpu,
            scaler,
        };
        if pictures == Pictures::Still {
            source.advance(target)?;
        } else {
            source.seek(target)?;
            source.advance(target)?;
        }
        Ok(source)
    }

    /// Opens `input` without decoding anything yet.
    fn bare(input: &Input, origin: f64, pictures: Pictures) -> Result<Source> {
        let reader = VideoReader::open_input(input)?;
        let tb = reader.time_base();
        Ok(Source {
            reader,
            tick: f64::from(tb.numerator()) / f64::from(tb.denominator()),
            origin,
            source_frame: 0,
            target: 0.0,
            current: None,
            normalized: None,
            pictures,
            on_gpu: false,
            scaler: None,
        })
    }

    fn seek(&mut self, target: f64) -> Result<()> {
        self.reader.seek((target / self.tick).floor() as i64)?;
        self.target = target;
        self.current = None;
        self.normalized = None;
        Ok(())
    }

    /// Moves `current` to the first frame at or after `target` (1 µs of slack
    /// for float noise). Past the end of a file it stays on the last frame,
    /// which is how a still keeps its one picture, and how a sequence's last
    /// picture lasts its frame (a project rate above the sequence's asks for
    /// a time past it). A sequence that ends before its last picture fails:
    /// a file is missing, and repeating the picture before it would hide the
    /// hole. Returns whether it moved.
    fn advance(&mut self, target: f64) -> Result<bool> {
        self.target = target;
        let mut changed = false;
        while self
            .current
            .as_ref()
            .is_none_or(|(_, t)| *t < target - 1e-6)
        {
            match self.reader.next_frame()? {
                Some(f) => {
                    let t = f.pts().unwrap_or(0) as f64 * self.tick;
                    self.current = Some((f, t));
                    changed = true;
                }
                None => {
                    // Picture `k` of a sequence has timestamp `k`.
                    let shown = self.current.as_ref().map(|(f, _)| f.pts().unwrap_or(0));
                    let missing = match (shown, self.pictures) {
                        (None, _) => String::new(),
                        (Some(k), Pictures::Sequence { count }) if k + 1 < count as i64 => {
                            let first = match self.reader.input() {
                                Input::Sequence { first, .. } => *first,
                                Input::File(_) => 0,
                            };
                            format!(
                                ": picture {} of {count} (file number {}) is missing",
                                k + 1,
                                first + k as u64 + 1
                            )
                        }
                        // The last picture: of a video or a still, or of a
                        // whole sequence.
                        (Some(_), _) => break,
                    };
                    return Err(Error::Invalid(format!(
                        "{} ends before {target:.3} s{missing}",
                        self.reader.path().display()
                    )));
                }
            }
        }
        Ok(changed)
    }

    /// Readies the file for another piece starting at `target`: a piece that
    /// starts a little ahead decodes on to it, any other seeks. A still is
    /// never sought; its one picture already stands in for every frame.
    fn reposition(&mut self, origin: f64, source_frame: i64, target: f64) -> Result<()> {
        self.origin = origin;
        self.source_frame = source_frame;
        if self.pictures == Pictures::Still {
            return Ok(());
        }
        let ahead = self
            .current
            .as_ref()
            .is_some_and(|(_, t)| target >= self.target && target - t <= DECODE_AHEAD);
        if !ahead {
            self.seek(target)?;
        }
        Ok(())
    }
}

/// Produces the plan's video frames in order. A piece from the same file as
/// the one before it reads on with the same decoder; a piece from another
/// file is opened while the piece before it encodes.
pub struct VideoFrames {
    pieces: Vec<VideoPiece>,
    fps: Rational,
    out: (u32, u32, format::Pixel),
    piece: usize,
    /// Frame index within the current piece.
    k: i64,
    source: Option<Source>,
    ahead: Ahead<Source>,
    normalizer: Option<Normalizer>,
    black: Option<frame::Video>,
    /// The output's dynamic range (`None`: SDR BT.709).
    target: Option<Hdr>,
    /// Frames for NVENC in GPU memory (`with_gpu`).
    gpu: Option<Gpu>,
}

/// The GPU side of an export whose frames stay there.
struct Gpu {
    side: GpuSide,
    /// Where pictures made on the CPU, black among them, go up.
    frames: crate::cuda::CudaFrames,
    /// Frames decoded and scaled on the GPU so far.
    made: i64,
}

impl VideoFrames {
    /// `target`: the output's dynamic range (`None`: SDR BT.709).
    pub fn new(
        pieces: &[VideoPiece],
        fps: Rational,
        width: u32,
        height: u32,
        pixel: format::Pixel,
        target: Option<Hdr>,
    ) -> Self {
        Self {
            pieces: pieces.to_vec(),
            fps,
            out: (width, height, pixel),
            piece: 0,
            k: 0,
            source: None,
            ahead: Ahead::new(),
            normalizer: None,
            black: None,
            target,
            gpu: None,
        }
    }

    /// Every frame in GPU memory, from `frames` (made for this output's
    /// size and format, which must be NV12 or P010): pieces the GPU shows
    /// as the CPU would decode on NVDEC and scale there; the rest, and
    /// black, are made on the CPU and go up.
    pub fn with_gpu(
        mut self,
        device: crate::cuda::CudaDevice,
        frames: crate::cuda::CudaFrames,
    ) -> Self {
        self.gpu = Some(Gpu {
            side: GpuSide {
                device,
                out: (self.out.0, self.out.1),
                format: self.out.2,
            },
            frames,
            made: 0,
        });
        self
    }

    /// Frames decoded and scaled on the GPU so far.
    pub fn gpu_frames(&self) -> i64 {
        self.gpu.as_ref().map_or(0, |g| g.made)
    }

    /// The next output frame (pts unset), or `None` after the last one.
    pub fn next_frame(&mut self) -> Result<Option<frame::Video>> {
        loop {
            let Some(piece) = self.pieces.get(self.piece) else {
                return Ok(None);
            };
            if self.k >= piece.frames() {
                // The file stays open: a later piece may read it too.
                self.piece += 1;
                self.k = 0;
                continue;
            }
            let k = self.k;
            self.k += 1;
            return match piece.clone() {
                VideoPiece::Black { .. } => {
                    let black = frame_ref(self.black()?)?;
                    match &self.gpu {
                        Some(g) => g.frames.upload(&black).map(Some),
                        None => Ok(Some(black)),
                    }
                }
                VideoPiece::Source {
                    path,
                    origin,
                    source_frame,
                    image,
                    ..
                } => {
                    // A still's one picture serves every frame: it is never
                    // sought (an image file has nothing past it).
                    let pictures = Pictures::of(image.as_ref());
                    let still = pictures == Pictures::Still;
                    if k == 0 {
                        let from = if still { 0 } else { source_frame };
                        self.start_piece(&input_of(&path, image.as_ref()), origin, from, pictures)?;
                    }
                    self.source_frame(if still { 0 } else { k }).map(Some)
                }
            };
        }
    }

    /// The time (seconds) of the keyframe decoding starts from for the
    /// first frame of `piece`, a video file's: a seek and one decoded frame.
    /// `None` for black, stills and image sequences.
    pub fn keyframe_time(&mut self, piece: &VideoPiece) -> Result<Option<f64>> {
        let VideoPiece::Source {
            path,
            origin,
            source_frame,
            image: None,
            ..
        } = piece
        else {
            return Ok(None);
        };
        let input = input_of(path, None);
        let target = self.target(*origin, *source_frame);
        let mut source = match self.source.take() {
            Some(s) if s.reader.input() == &input => s,
            _ => Source::bare(&input, *origin, Pictures::Video)?,
        };
        source.seek(target)?;
        let at = source.reader.pending_pts().map(|p| p as f64 * source.tick);
        self.source = Some(source);
        Ok(at)
    }

    /// Makes `piece` alone the plan, its frames next: the open file stays
    /// open, so a piece of it starting shortly after the frame before
    /// decodes on to it instead of seeking.
    pub fn read_piece(&mut self, piece: &VideoPiece) {
        self.pieces = vec![piece.clone()];
        self.piece = 0;
        self.k = 0;
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

    /// Readies the current piece's file: the open one if it is the same
    /// file, the one opened ahead for this piece, or a new one. Then starts
    /// opening the next file.
    fn start_piece(
        &mut self,
        input: &Input,
        origin: f64,
        source_frame: i64,
        pictures: Pictures,
    ) -> Result<()> {
        let target = self.target(origin, source_frame);
        let source = match self.source.take() {
            Some(mut s) if s.reader.input() == input => {
                s.reposition(origin, source_frame, target)?;
                s
            }
            _ => match self.ahead.take(self.piece) {
                Some(opened) => opened?,
                None => Source::open(
                    input,
                    origin,
                    source_frame,
                    target,
                    pictures,
                    self.gpu.as_ref().map(|g| &g.side),
                )?,
            },
        };
        self.source = Some(source);
        self.open_next(input);
        Ok(())
    }

    /// Starts opening the file of the next source piece, unless it is
    /// `current`, which that piece then reads on with.
    fn open_next(&mut self, current: &Input) {
        let next = self
            .pieces
            .iter()
            .enumerate()
            .skip(self.piece + 1)
            .find_map(|(i, p)| match p {
                VideoPiece::Source {
                    path,
                    origin,
                    source_frame,
                    frames,
                    image,
                    ..
                } if *frames > 0 => {
                    let pictures = Pictures::of(image.as_ref());
                    let from = if pictures == Pictures::Still {
                        0
                    } else {
                        *source_frame
                    };
                    Some((i, input_of(path, image.as_ref()), *origin, from, pictures))
                }
                _ => None,
            });
        let Some((i, input, origin, source_frame, pictures)) = next else {
            return;
        };
        if &input == current {
            return;
        }
        let target = self.target(origin, source_frame);
        let gpu = self.gpu.as_ref().map(|g| g.side.clone());
        self.ahead.start(i, move || {
            Source::open(&input, origin, source_frame, target, pictures, gpu.as_ref())
        });
    }

    /// Output frame `k` of the current source piece.
    fn source_frame(&mut self, k: i64) -> Result<frame::Video> {
        let (out_w, out_h, out_fmt) = self.out;
        let fps = self.fps;
        let src = self.source.as_mut().expect("open");
        let target =
            src.origin + fps.frames_to_secs(src.source_frame + k) - 0.5 * fps.frames_to_secs(1);
        let changed = src.advance(target)?;
        if src.on_gpu && (changed || src.normalized.is_none()) {
            let (frame, _) = src.current.as_ref().expect("have a frame");
            let scaler = src.scaler.as_mut().expect("built at opening");
            scaler.push(Some(frame))?;
            let picture = scaler
                .pull()?
                .ok_or_else(|| Error::Invalid("the GPU scaler gave no picture".into()))?;
            src.normalized = Some(picture);
        } else if changed || src.normalized.is_none() {
            let (frame, _) = src.current.as_ref().expect("have a frame");
            let rotation = src.reader.rotation();
            let rebuild = self
                .normalizer
                .as_ref()
                .is_none_or(|n| !n.matches(frame, rotation));
            if rebuild {
                self.normalizer = Some(Normalizer::new(
                    frame,
                    rotation,
                    out_w,
                    out_h,
                    out_fmt,
                    self.target,
                )?);
            }
            src.normalized = Some(self.normalizer.as_mut().expect("built").run(frame)?);
        }
        let picture = frame_ref(src.normalized.as_ref().expect("normalized"))?;
        match &mut self.gpu {
            Some(g) if src.on_gpu => {
                g.made += 1;
                Ok(picture)
            }
            Some(g) => g.frames.upload(&picture),
            None => Ok(picture),
        }
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
            for px in data.as_chunks_mut::<2>().0 {
                *px = v.to_le_bytes();
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
