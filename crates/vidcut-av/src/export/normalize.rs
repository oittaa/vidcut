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

/// The size `src` is shown at inside an `out_w`×`out_h` picture, as the
/// normalizer fits it upright (turned by `rotation`), in the frame's own
/// orientation.
pub(crate) fn shown_size(
    src: &frame::Video,
    rotation: u16,
    (out_w, out_h): (u32, u32),
) -> (u32, u32) {
    let (w, h) = (src.width(), src.height());
    let sar = src.aspect_ratio();
    let sar = (sar.numerator(), sar.denominator());
    if rotation % 180 == 90 {
        let (sw, sh) = fit(h, w, (sar.1, sar.0), out_w, out_h);
        (sh, sw)
    } else {
        fit(w, h, sar, out_w, out_h)
    }
}

/// A frame in GPU memory brought down for the CPU to normalize into an
/// `out` picture: scaled on the GPU first to the size it is shown at there
/// ([`shown_size`], its pixels square) when that is smaller, so a fraction
/// comes down. The CPU does the rest as for any frame: turning, colors,
/// HDR.
pub(crate) fn down(
    shrinker: &mut Option<crate::hw::Shrinker>,
    src: &frame::Video,
    rotation: u16,
    out: (u32, u32),
) -> Result<frame::Video> {
    let size = shown_size(src, rotation, out);
    let shrinks = size.0 < src.width() && size.1 < src.height();
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
        let color = crate::color::Source::of(src);
        let mut graph = filter::Graph::new();
        let mut args = format!(
            "video_size={w}x{h}:pix_fmt={}:time_base=1/1000000:pixel_aspect={}/{}",
            ffmpeg::ffi::AVPixelFormat::from(fmt) as i32,
            sar.0.max(1),
            sar.1.max(1)
        );
        // The graph is told the matrix and range the picture is read with,
        // and so is each frame (`run`). Then a conversion FFmpeg adds by
        // itself, for a filter that doesn't take the layout (4:2:2 before
        // `transpose`), keeps them. Told nothing, it converts the picture
        // to BT.601's matrix.
        if let Some((matrix, range)) = color.yuv_tags() {
            args += &format!(":colorspace={}:range={}", matrix as i32, range as i32);
        }
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
        // The matrix and range the graph was told (`with`).
        if let Some((matrix, range)) = self.input.5.yuv_tags() {
            input.set_color_space(matrix.into());
            input.set_color_range(range.into());
        }
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

/// The layouts a display takes: 4:2:0 in 8 bits (planar, or NV12 as
/// hardware decoders give it) and in 10 (P010, or planar as the CPU's
/// decoders give it), 4:2:2 in 8 and 10 bits planar, and in 10
/// interleaved (P210, as VideoToolbox gives ProRes), and 4:4:4 in 12 bits
/// planar (ProRes 4444, its alpha left out) and in 16 interleaved (P416,
/// as VideoToolbox gives ProRes 4444).
pub(crate) fn displayable(p: format::Pixel) -> bool {
    use format::Pixel as P;
    matches!(
        p,
        P::YUV420P
            | P::YUVJ420P
            | P::NV12
            | P::P010LE
            | P::YUV420P10LE
            | P::YUV422P
            | P::YUVJ422P
            | P::YUV422P10LE
            | P::P210LE
            | P::YUV444P12LE
            | P::YUVA444P12LE
            | P::P416LE
    )
}

/// Brings decoded pictures into a layout a display takes, at their own size
/// and in their own colors. Pictures in other layouts, such as 4:4:4 in 8
/// or 10 bits, are resampled to 4:2:0 (8 bits, or P010 for more), keeping
/// their matrix and range. RGB pictures (images) are coded as BT.709 video
/// range. Pictures already in such a layout pass as they are, by
/// reference.
pub(crate) struct Relayout {
    /// The layouts that pass.
    passes: fn(format::Pixel) -> bool,
    graph: Option<(filter::Graph, (u32, u32, format::Pixel))>,
}

impl Relayout {
    /// Into a layout a display takes ([`displayable`]).
    pub(crate) fn new() -> Self {
        Self::passing(displayable)
    }

    /// Into a layout `passes` takes, which must take 8-bit 4:2:0 and P010:
    /// what the pictures in other layouts become.
    pub(crate) fn passing(passes: fn(format::Pixel) -> bool) -> Self {
        Self {
            passes,
            graph: None,
        }
    }

    pub(crate) fn run(&mut self, src: &frame::Video) -> Result<frame::Video> {
        let fmt = src.format();
        if (self.passes)(fmt) {
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
    use super::{Normalizer, Relayout, displayable, fit, shown_size};
    use crate::ffmpeg::{color, format::Pixel, frame};

    #[test]
    fn a_display_takes_10_bit_planar_4_2_0() {
        assert!(displayable(Pixel::YUV420P10LE));
    }

    #[test]
    fn a_display_takes_8_bit_planar_4_2_2() {
        assert!(displayable(Pixel::YUV422P));
    }

    #[test]
    fn a_display_takes_full_range_8_bit_planar_4_2_2() {
        assert!(displayable(Pixel::YUVJ422P));
    }

    #[test]
    fn a_display_takes_10_bit_planar_4_2_2() {
        assert!(displayable(Pixel::YUV422P10LE));
    }

    #[test]
    fn a_display_takes_10_bit_interleaved_4_2_2() {
        assert!(displayable(Pixel::P210LE));
    }

    #[test]
    fn a_display_takes_12_bit_planar_4_4_4() {
        assert!(displayable(Pixel::YUV444P12LE));
    }

    #[test]
    fn a_display_takes_12_bit_planar_4_4_4_with_alpha() {
        assert!(displayable(Pixel::YUVA444P12LE));
    }

    #[test]
    fn a_display_takes_16_bit_interleaved_4_4_4() {
        assert!(displayable(Pixel::P416LE));
    }

    #[test]
    fn a_display_doesnt_take_8_bit_4_4_4() {
        assert!(!displayable(Pixel::YUV444P));
    }

    #[test]
    fn a_picture_brought_to_4_2_0_keeps_its_matrix_and_range() {
        // An orange in BT.709 video range, 4:4:4. Told nothing, swscale
        // would convert it to BT.601's numbers.
        let mut f = frame::Video::new(Pixel::YUV444P, 64, 64);
        f.set_color_space(color::Space::BT709);
        f.set_color_range(color::Range::MPEG);
        f.set_pts(Some(0));
        for (plane, v) in [(0, 100), (1, 90), (2, 170)] {
            f.data_mut(plane).fill(v);
        }
        let out = Relayout::new().run(&f).unwrap();
        let samples = |plane: usize, side: usize| -> Vec<u8> {
            let stride = out.stride(plane);
            (0..side)
                .flat_map(|y| out.data(plane)[y * stride..y * stride + side].to_vec())
                .collect()
        };
        assert_eq!(out.format(), Pixel::YUV420P);
        assert_eq!(samples(0, 64), vec![100; 64 * 64]);
        assert_eq!(samples(1, 32), vec![90; 32 * 32]);
        assert_eq!(samples(2, 32), vec![170; 32 * 32]);
    }

    #[test]
    fn a_turned_frame_is_shown_at_a_size_in_its_own_orientation() {
        let phone = frame::Video::new(Pixel::YUV420P, 3840, 2160);
        assert_eq!(shown_size(&phone, 90, (540, 960)), (960, 540));
    }

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

    /// A flat orange (Y 100, Cb 90, Cr 170 in 8 bits), 64×64 in `pixel`,
    /// tagged `matrix` and video range, turned a quarter and converted to
    /// RGB: each pixel's.
    fn turned_orange(pixel: Pixel, matrix: color::Space) -> Vec<[u8; 3]> {
        let mut f = frame::Video::new(pixel, 64, 64);
        f.set_color_space(matrix);
        f.set_color_range(color::Range::MPEG);
        f.set_pts(Some(0));
        for (plane, v) in [(0, 100u16), (1, 90), (2, 170)] {
            if pixel == Pixel::YUV422P10LE {
                for px in f.data_mut(plane).as_chunks_mut::<2>().0 {
                    *px = (v << 2).to_le_bytes();
                }
            } else {
                f.data_mut(plane).fill(v as u8);
            }
        }
        let out = Normalizer::new(&f, 90, 64, 64, Pixel::RGBA, None)
            .and_then(|mut n| n.run(&f))
            .unwrap();
        let (stride, data) = (out.stride(0), out.data(0));
        (0..64)
            .flat_map(|y| (0..64).map(move |x| y * stride + 4 * x))
            .map(|at| [data[at], data[at + 1], data[at + 2]])
            .collect()
    }

    /// How far `rgb`'s pixels are from `want` at most, in levels of a
    /// channel.
    fn farthest(rgb: &[[u8; 3]], want: [u8; 3]) -> u8 {
        rgb.iter()
            .flat_map(|p| (0..3).map(move |c| p[c].abs_diff(want[c])))
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn an_8_bit_4_2_2_picture_turned_a_quarter_keeps_its_matrix() {
        let rgb = turned_orange(Pixel::YUV422P, color::Space::BT709);
        assert!(farthest(&rgb, [173, 84, 18]) <= 1, "{:?}", rgb[0]);
    }

    #[test]
    fn a_10_bit_4_2_2_picture_turned_a_quarter_keeps_its_matrix() {
        let rgb = turned_orange(Pixel::YUV422P10LE, color::Space::BT709);
        assert!(farthest(&rgb, [173, 84, 18]) <= 1, "{:?}", rgb[0]);
    }

    #[test]
    fn a_turned_4_2_2_picture_tagged_smpte_240m_is_read_as_untagged() {
        // Untagged at this size: BT.601.
        let rgb = turned_orange(Pixel::YUV422P, color::Space::SMPTE240M);
        assert!(farthest(&rgb, [165, 79, 21]) <= 1, "{:?}", rgb[0]);
    }
}
