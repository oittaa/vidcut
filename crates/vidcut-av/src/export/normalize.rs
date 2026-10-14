//! Pictures made in memory: the CPU's fitting of a decoded frame into the
//! output (`Normalizer`), the layout a display takes (`Relayout`), the
//! geometry (`fit`), and a GPU frame's way down for them (`down`).

use vidcut_core::model::Hdr;

use super::video::frame_ref;
use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, filter, format, frame};

/// Turns pictures upright (the stream's rotation), scales them keeping the
/// display aspect (honoring the sample aspect ratio), pads to the output
/// size, and converts them to the output's colors and pixel format.
pub(crate) struct Normalizer {
    graph: filter::Graph,
    /// Without `zscale`: its math on the CPU ([`crate::color::split`]).
    cpu: Option<CpuStage>,
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

/// `zscale`'s part of a [`Normalizer`] without it: the colors converted on
/// the CPU, written as planar YUV there (`write`) or not, and the graph from
/// there into the output's format (`back`).
struct CpuStage {
    colors: crate::color::CpuColors,
    write: Option<format::Pixel>,
    target: Option<Hdr>,
    back: Option<filter::Graph>,
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

/// A frame in GPU memory brought down for the CPU to normalize into an
/// `out_w`×`out_h` picture: scaled on the GPU first to the size it is shown
/// at there (in the frame's own orientation and layout, its pixels square)
/// when that is smaller, so a fraction comes down. The CPU does the rest as
/// for any frame: turning, colors, HDR.
pub(crate) fn down(
    shrinker: &mut Option<crate::hw::Shrinker>,
    src: &frame::Video,
    rotation: u16,
    (out_w, out_h): (u32, u32),
) -> Result<frame::Video> {
    let (w, h) = (src.width(), src.height());
    let sar = src.aspect_ratio();
    let sar = (sar.numerator(), sar.denominator());
    // As the normalizer fits it, upright; then in the frame's orientation.
    let size = if rotation % 180 == 90 {
        let (sw, sh) = fit(h, w, (sar.1, sar.0), out_w, out_h);
        (sh, sw)
    } else {
        fit(w, h, sar, out_w, out_h)
    };
    let shrinks = size.0 < w && size.1 < h;
    if !shrinks {
        return crate::hw::download(src);
    }
    let mut small = crate::hw::Shrinker::down(shrinker, src, size)?;
    // Unless it came down whole.
    if (small.width(), small.height()) == size {
        // SAFETY: a plain field of its own frame.
        unsafe {
            (*small.as_mut_ptr()).sample_aspect_ratio = ffmpeg::ffi::AVRational { num: 1, den: 1 }
        };
    }
    Ok(small)
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
        let zscale = crate::parts::has_filter("zscale");
        Self::with(src, rotation, (out_w, out_h), out_fmt, target, zscale)
    }

    /// [`Normalizer::new`], converting colors with `zscale` or, without
    /// it, on the CPU.
    fn with(
        src: &frame::Video,
        rotation: u16,
        (out_w, out_h): (u32, u32),
        out_fmt: format::Pixel,
        target: Option<Hdr>,
        zscale: bool,
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
        // Without zimg the CPU does `zscale`'s part, between two graphs.
        let (conv, cpu) = if crate::color::needs_zscale(color, target) && !zscale {
            let split = crate::color::split(color, out_fmt, target);
            (split.before, Some((split.cpu, split.write, split.back)))
        } else {
            (crate::color::conversion(color, out_fmt, target), None)
        };
        let last = match cpu {
            Some(_) => String::new(),
            None => format!(",format={}", fmt_name(out_fmt)),
        };
        let spec = format!(
            "{}scale={sw}:{sh}:flags={}{},pad={out_w}:{out_h}:{}:{}:color=black,setsar=1{}{last}",
            upright(rotation),
            conv.flags,
            conv.scale,
            (out_w - sw) / 2,
            (out_h - sh) / 2,
            conv.after,
        );
        crate::parts::parse_filters(&mut graph, &spec)?;
        graph
            .validate()
            .ctx(|| format!("configuring filter {spec}"))?;
        let cpu = match cpu {
            Some((colors, write, back)) => {
                let from = write.unwrap_or(format::Pixel::GBRPF32LE);
                let args = format!(
                    "video_size={out_w}x{out_h}:pix_fmt={}:time_base=1/1000000:pixel_aspect=1/1",
                    ffmpeg::ffi::AVPixelFormat::from(from) as i32
                );
                let back = match back {
                    Some(spec) => Some(simple_graph(&args, &spec)?),
                    None => None,
                };
                Some(CpuStage {
                    colors,
                    write,
                    target,
                    back,
                })
            }
            None => None,
        };
        Ok(Self {
            graph,
            cpu,
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
        let Some(cpu) = self.cpu.as_mut() else {
            return Ok(out);
        };
        // SAFETY: the graph's frame, made the caller's own to write.
        if unsafe { ffmpeg::ffi::av_frame_make_writable(out.as_mut_ptr()) } < 0 {
            return Err(Error::Invalid("no memory for a frame".into()));
        }
        cpu.colors.run(&mut out);
        if let Some(pixel) = cpu.write {
            let mut yuv = frame::Video::new(pixel, out.width(), out.height());
            crate::color::write_yuv(&out, &mut yuv, cpu.target);
            yuv.set_pts(out.pts());
            out = yuv;
        }
        let Some(back) = cpu.back.as_mut() else {
            return Ok(out);
        };
        back.get("in")
            .expect("source")
            .source()
            .add(&out)
            .ctx(|| "filtering frame".into())?;
        let mut written = frame::Video::empty();
        back.get("out")
            .expect("sink")
            .sink()
            .frame(&mut written)
            .ctx(|| "filtering frame".into())?;
        Ok(written)
    }
}

/// A graph of `spec` between a source of `args` and a sink.
fn simple_graph(args: &str, spec: &str) -> Result<filter::Graph> {
    let find =
        |name: &str| filter::find(name).ok_or_else(|| Error::Invalid(format!("no {name} filter")));
    let mut graph = filter::Graph::new();
    graph
        .add(&find("buffer")?, "in", args)
        .ctx(|| "creating video source".into())?;
    graph
        .add(&find("buffersink")?, "out", "")
        .ctx(|| "creating video sink".into())?;
    crate::parts::parse_filters(&mut graph, spec)?;
    graph
        .validate()
        .ctx(|| format!("configuring filter {spec}"))?;
    Ok(graph)
}

/// The layouts a display takes: 8-bit 4:2:0 (planar, or NV12 as hardware
/// decoders give it) and P010 for more bits.
pub(crate) fn displayable(p: format::Pixel) -> bool {
    use format::Pixel as P;
    matches!(p, P::YUV420P | P::YUVJ420P | P::NV12 | P::P010LE)
}

/// Brings decoded pictures into a layout a display takes, at their own size
/// and in their own colors: 4:2:2, 4:4:4 and 10-bit planar pictures are
/// resampled to 4:2:0 (8 bits, or P010 for more), keeping their matrix and
/// range; RGB pictures (images) are coded as BT.709 video range. Pictures
/// already in such a layout pass as they are, by reference.
pub(crate) struct Relayout {
    graph: Option<(filter::Graph, (u32, u32, format::Pixel))>,
}

impl Relayout {
    pub(crate) fn new() -> Self {
        Self { graph: None }
    }

    pub(crate) fn run(&mut self, src: &frame::Video) -> Result<frame::Video> {
        let fmt = src.format();
        if displayable(fmt) {
            return frame_ref(src);
        }
        let key = (src.width(), src.height(), fmt);
        if self.graph.as_ref().is_none_or(|(_, k)| *k != key) {
            self.graph = Some((Self::build(src)?, key));
        }
        let (graph, _) = self.graph.as_mut().expect("built");
        graph
            .get("in")
            .expect("source")
            .source()
            .add(src)
            .ctx(|| "converting a picture's layout".into())?;
        let mut out = frame::Video::empty();
        graph
            .get("out")
            .expect("sink")
            .sink()
            .frame(&mut out)
            .ctx(|| "converting a picture's layout".into())?;
        Ok(out)
    }

    fn build(src: &frame::Video) -> Result<filter::Graph> {
        let desc = src
            .format()
            .descriptor()
            .ok_or_else(|| Error::Invalid("unknown pixel format".into()))?;
        let (depth, rgb) = unsafe {
            let d = &*desc.as_ptr();
            (
                d.comp[0].depth,
                d.flags & (ffmpeg::ffi::AV_PIX_FMT_FLAG_RGB as u64) != 0,
            )
        };
        let target = if depth > 8 { "p010le" } else { "yuv420p" };
        // A YUV picture keeps its matrix and range, named on both sides:
        // told nothing, swscale would convert to BT.601. RGB is converted,
        // with the flags that round its colors right.
        let spec = if rgb {
            let flags = crate::color::CONVERT;
            format!("scale=out_color_matrix=bt709:out_range=limited:flags={flags},format={target}")
        } else {
            format!("scale={},format={target}", crate::color::keep(src))
        };
        let sar = src.aspect_ratio();
        let args = format!(
            "video_size={}x{}:pix_fmt={}:time_base=1/1000000:pixel_aspect={}/{}",
            src.width(),
            src.height(),
            ffmpeg::ffi::AVPixelFormat::from(src.format()) as i32,
            sar.numerator().max(1),
            sar.denominator().max(1)
        );
        let find = |name: &str| {
            filter::find(name).ok_or_else(|| Error::Invalid(format!("no {name} filter")))
        };
        let mut graph = filter::Graph::new();
        graph
            .add(&find("buffer")?, "in", &args)
            .ctx(|| "creating video source".into())?;
        graph
            .add(&find("buffersink")?, "out", "")
            .ctx(|| "creating video sink".into())?;
        crate::parts::parse_filters(&mut graph, &spec)?;
        graph
            .validate()
            .ctx(|| format!("configuring filter {spec}"))?;
        Ok(graph)
    }
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
