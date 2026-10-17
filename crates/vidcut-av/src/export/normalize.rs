use vidcut_core::model::Hdr;

use super::video::frame_ref;
use crate::decode::Reading;
use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, filter, format, frame};

pub(crate) struct Normalizer {
    graph: filter::Graph,
    cpu: Option<CpuStage>,
    input: (
        u32,
        u32,
        format::Pixel,
        (i32, i32),
        u16,
        crate::color::Source,
    ),
}

struct CpuStage {
    colors: crate::color::CpuColors,
    write: Option<format::Pixel>,
    target: Option<Hdr>,
    back: Option<filter::Graph>,
}

fn upright(rotation: u16) -> &'static str {
    match rotation {
        90 => "transpose=clock,",
        180 => "hflip,vflip,",
        270 => "transpose=cclock,",
        _ => "",
    }
}

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
    if (small.width(), small.height()) == size {
        // SAFETY: a plain field of its own frame.
        unsafe {
            (*small.as_mut_ptr()).sample_aspect_ratio = ffmpeg::ffi::AVRational { num: 1, den: 1 }
        };
    }
    Ok(small)
}

impl Normalizer {
    pub(crate) fn new(
        src: &frame::Video,
        rotation: u16,
        out_w: u32,
        out_h: u32,
        out_fmt: format::Pixel,
        target: Option<Hdr>,
        reading: Reading,
    ) -> Result<Self> {
        let zscale = crate::parts::has_filter("zscale");
        Self::with(
            src,
            rotation,
            (out_w, out_h),
            out_fmt,
            target,
            zscale,
            reading,
        )
    }

    fn with(
        src: &frame::Video,
        rotation: u16,
        (out_w, out_h): (u32, u32),
        out_fmt: format::Pixel,
        target: Option<Hdr>,
        zscale: bool,
        reading: Reading,
    ) -> Result<Self> {
        let (w, h, fmt) = (src.width(), src.height(), src.format());
        let sar = src.aspect_ratio();
        let sar = (sar.numerator(), sar.denominator());
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
        let mut graph = crate::pool::graph(reading);
        let mut args = format!(
            "video_size={w}x{h}:pix_fmt={}:time_base=1/1000000:pixel_aspect={}/{}",
            ffmpeg::ffi::AVPixelFormat::from(fmt) as i32,
            sar.0.max(1),
            sar.1.max(1)
        );
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
                    Some(spec) => Some(simple_graph(&args, &spec, reading)?),
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

fn simple_graph(args: &str, spec: &str, reading: Reading) -> Result<filter::Graph> {
    let find =
        |name: &str| filter::find(name).ok_or_else(|| Error::Invalid(format!("no {name} filter")));
    let mut graph = crate::pool::graph(reading);
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

pub(crate) struct Relayout {
    passes: fn(format::Pixel) -> bool,
    graph: Option<(filter::Graph, (u32, u32, format::Pixel))>,
}

impl Relayout {
    pub(crate) fn new() -> Self {
        Self::passing(displayable)
    }

    pub(crate) fn passing(passes: fn(format::Pixel) -> bool) -> Self {
        Self {
            passes,
            graph: None,
        }
    }

    pub(crate) fn run(&mut self, src: &frame::Video, reading: Reading) -> Result<frame::Video> {
        let fmt = src.format();
        if (self.passes)(fmt) {
            return frame_ref(src);
        }
        let key = (src.width(), src.height(), fmt);
        if self.graph.as_ref().is_none_or(|(_, k)| *k != key) {
            self.graph = Some((Self::build(src, reading)?, key));
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

    fn build(src: &frame::Video, reading: Reading) -> Result<filter::Graph> {
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
        let mut graph = crate::pool::graph(reading);
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
    use crate::decode::Reading;
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
        let mut f = frame::Video::new(Pixel::YUV444P, 64, 64);
        f.set_color_space(color::Space::BT709);
        f.set_color_range(color::Range::MPEG);
        f.set_pts(Some(0));
        for (plane, v) in [(0, 100), (1, 90), (2, 170)] {
            f.data_mut(plane).fill(v);
        }
        let out = Relayout::new().run(&f, Reading::Stream).unwrap();
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
        let out = Normalizer::new(&f, 90, 64, 64, Pixel::RGBA, None, Reading::Stream)
            .and_then(|mut n| n.run(&f))
            .unwrap();
        let (stride, data) = (out.stride(0), out.data(0));
        (0..64)
            .flat_map(|y| (0..64).map(move |x| y * stride + 4 * x))
            .map(|at| [data[at], data[at + 1], data[at + 2]])
            .collect()
    }

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
        let rgb = turned_orange(Pixel::YUV422P, color::Space::SMPTE240M);
        assert!(farthest(&rgb, [165, 79, 21]) <= 1, "{:?}", rgb[0]);
    }
}
