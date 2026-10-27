pub(crate) use vidcut_core::place::fit;

use super::video::frame_ref;
use crate::decode::Reading;
use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, filter, format, frame};

pub(crate) fn has_alpha(p: format::Pixel) -> bool {
    p.descriptor().is_some_and(|d| {
        // SAFETY: FFmpeg's static descriptor of the format.
        let flags = unsafe { (*d.as_ptr()).flags };
        flags & ffmpeg::ffi::AV_PIX_FMT_FLAG_ALPHA as u64 != 0
    })
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

pub(crate) fn enlarges(
    (w, h): (u32, u32),
    (n, d): (i32, i32),
    rotation: u16,
    (fw, fh): (u32, u32),
) -> bool {
    let across = if n > 0 && d > 0 {
        f64::from(n) / f64::from(d)
    } else {
        1.0
    };
    let (sw, sh) = (f64::from(w) * across, f64::from(h));
    let (sw, sh) = if rotation % 180 == 90 {
        (sh, sw)
    } else {
        (sw, sh)
    };
    f64::from(fw) > sw + 0.5 || f64::from(fh) > sh + 0.5
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
            | P::AYUV64LE
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
        let target = match (has_alpha(src.format()), depth > 8) {
            (true, _) => "yuva444p12le",
            (false, true) => "p010le",
            (false, false) => "yuv420p",
        };
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
    use super::{Relayout, displayable, fit, shown_size};
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
}
