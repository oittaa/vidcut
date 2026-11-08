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

pub(crate) fn shown_size(src: &frame::Video, rotation: u16, out: (u32, u32)) -> (u32, u32) {
    let sar = src.aspect_ratio();
    let sar = (sar.numerator(), sar.denominator());
    shown_in((src.width(), src.height()), sar, rotation, out)
}

pub(crate) fn shown_in(
    (w, h): (u32, u32),
    sar: (i32, i32),
    rotation: u16,
    (out_w, out_h): (u32, u32),
) -> (u32, u32) {
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
            | P::RGB24
            | P::GBRP16LE
            | P::GBRAP16LE
            | P::GBRPF16LE
            | P::GBRAPF16LE
            | P::GBRPF32LE
            | P::GBRAPF32LE
    )
}

fn spread_to(p: format::Pixel) -> Option<format::Pixel> {
    use format::Pixel as P;
    match p {
        P::GRAYF16LE => Some(P::GBRPF16LE),
        P::YAF16LE => Some(P::GBRAPF16LE),
        P::GRAYF32LE => Some(P::GBRPF32LE),
        P::YAF32LE => Some(P::GBRAPF32LE),
        _ => None,
    }
}

fn spread(src: &frame::Video, to: format::Pixel) -> Result<frame::Video> {
    let size = match to {
        format::Pixel::GBRPF16LE | format::Pixel::GBRAPF16LE => 2,
        _ => 4,
    };
    let step = if has_alpha(to) { 2 * size } else { size };
    let (w, h) = (src.width() as usize, src.height() as usize);
    let mut out = frame::Video::new(to, src.width(), src.height());
    // SAFETY: two live frames; FFmpeg copies the fields that are not pictures.
    let ret = unsafe { ffmpeg::ffi::av_frame_copy_props(out.as_mut_ptr(), src.as_ptr()) };
    if ret < 0 {
        return Err(Error::Ffmpeg {
            context: "copying a gray picture's fields".into(),
            source: ffmpeg::Error::from(ret),
        });
    }
    let (from, stride) = (src.data(0), src.stride(0));
    for plane in 0..out.planes() {
        let at = if plane == 3 { size } else { 0 };
        let row = out.stride(plane);
        let to = out.data_mut(plane);
        for y in 0..h {
            let line = &from[y * stride..y * stride + w * step];
            let samples = to[y * row..y * row + w * size].chunks_exact_mut(size);
            for (sample, pixel) in samples.zip(line.chunks_exact(step)) {
                sample.copy_from_slice(&pixel[at..at + size]);
            }
        }
    }
    Ok(out)
}

pub(crate) struct Relayout {
    passes: fn(format::Pixel) -> bool,
    keeps_chroma: bool,
    graph: Option<(filter::Graph, (u32, u32, format::Pixel))>,
}

impl Relayout {
    pub(crate) fn new() -> Self {
        Self {
            keeps_chroma: true,
            ..Self::passing(displayable)
        }
    }

    pub(crate) fn passing(passes: fn(format::Pixel) -> bool) -> Self {
        Self {
            passes,
            keeps_chroma: false,
            graph: None,
        }
    }

    pub(crate) fn run(&mut self, src: &frame::Video, reading: Reading) -> Result<frame::Video> {
        let fmt = src.format();
        if (self.passes)(fmt) {
            return frame_ref(src);
        }
        if let Some(to) = spread_to(fmt).filter(|&to| (self.passes)(to)) {
            return spread(src, to);
        }
        let key = (src.width(), src.height(), fmt);
        if self.graph.as_ref().is_none_or(|(_, k)| *k != key) {
            self.graph = Some((Self::build(src, reading, self.keeps_chroma)?, key));
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

    fn build(src: &frame::Video, reading: Reading, keeps_chroma: bool) -> Result<filter::Graph> {
        let desc = src
            .format()
            .descriptor()
            .ok_or_else(|| Error::Invalid("unknown pixel format".into()))?;
        let (depth, rgb, gray, chroma) = unsafe {
            let d = &*desc.as_ptr();
            (
                d.comp[0].depth,
                d.flags & (ffmpeg::ffi::AV_PIX_FMT_FLAG_RGB as u64) != 0,
                d.nb_components <= 2 && d.flags & (ffmpeg::ffi::AV_PIX_FMT_FLAG_FLOAT as u64) == 0,
                (d.log2_chroma_w, d.log2_chroma_h),
            )
        };
        let alpha = has_alpha(src.format());
        let full = rgb || chroma == (0, 0) || depth > 10;
        let target = match (alpha, keeps_chroma, full, chroma, depth > 8) {
            (true, ..) => "yuva444p12le",
            (false, true, true, ..) => "yuv444p12le",
            (false, true, false, (1, 0), true) => "yuv422p10le",
            (false, true, false, (1, 0), false) => "yuv422p",
            (false, _, _, _, true) => "p010le",
            (false, _, _, _, false) => "yuv420p",
        };
        let flags = crate::color::CONVERT;
        let (wide, planar) = match (alpha, gray) {
            (true, _) => ("rgba64le", "gbrap16le"),
            (false, true) => ("gray16le", "gbrp16le"),
            (false, false) => ("rgb48le", "gbrp16le"),
        };
        let spec = match (rgb || gray, depth) {
            (true, 16..) => format!("scale=flags={flags},format={planar}"),
            (true, 9..16) => {
                format!("scale=flags={flags},format={wide},scale=flags={flags},format={planar}")
            }
            _ if rgb => format!(
                "scale=out_color_matrix=bt709:out_range=limited:flags={flags},format={target}"
            ),
            _ => format!("scale={},format={target}", crate::color::keep(src)),
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

    fn filled<const N: usize>(format: Pixel, planes: &[[u8; N]]) -> frame::Video {
        let mut f = frame::Video::new(format, 2, 2);
        f.set_pts(Some(0));
        for (plane, sample) in planes.iter().enumerate() {
            for s in f.data_mut(plane).as_chunks_mut::<N>().0 {
                *s = *sample;
            }
        }
        f
    }

    fn samples<const N: usize>(f: &frame::Video, plane: usize) -> Vec<[u8; N]> {
        let stride = f.stride(plane);
        (0..2)
            .flat_map(|y| {
                f.data(plane)[y * stride..y * stride + 2 * N]
                    .as_chunks::<N>()
                    .0
            })
            .copied()
            .collect()
    }

    fn floats(f: &frame::Video, plane: usize) -> Vec<f32> {
        samples::<4>(f, plane)
            .into_iter()
            .map(f32::from_le_bytes)
            .collect()
    }

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
    fn a_display_takes_planar_32_bit_floats_with_alpha() {
        assert!(displayable(Pixel::GBRAPF32LE));
    }

    #[test]
    fn a_display_takes_planar_half_floats() {
        assert!(displayable(Pixel::GBRPF16LE));
    }

    #[test]
    fn a_float_picture_keeps_its_values_above_1_and_below_0() {
        let planes = [0.5f32, -0.25, 2.5].map(f32::to_le_bytes);
        let out = Relayout::new()
            .run(&filled(Pixel::GBRPF32LE, &planes), Reading::Stream)
            .unwrap();
        assert_eq!(
            [floats(&out, 0), floats(&out, 1), floats(&out, 2)],
            [vec![0.5; 4], vec![-0.25; 4], vec![2.5; 4]]
        );
    }

    #[test]
    fn a_gray_float_picture_becomes_three_equal_float_planes() {
        let out = Relayout::new()
            .run(
                &filled(Pixel::GRAYF32LE, &[2.5f32.to_le_bytes()]),
                Reading::Stream,
            )
            .unwrap();
        assert_eq!(
            (
                out.format(),
                [floats(&out, 0), floats(&out, 1), floats(&out, 2)]
            ),
            (Pixel::GBRPF32LE, [vec![2.5; 4], vec![2.5; 4], vec![2.5; 4]])
        );
    }

    #[test]
    fn a_gray_half_float_picture_keeps_its_alpha_in_the_fourth_plane() {
        let minus_quarter_and_half = [0x00, 0xb4, 0x00, 0x38];
        let out = Relayout::new()
            .run(
                &filled(Pixel::YAF16LE, &[minus_quarter_and_half]),
                Reading::Stream,
            )
            .unwrap();
        assert_eq!(
            (out.format(), samples::<2>(&out, 2), samples::<2>(&out, 3)),
            (
                Pixel::GBRAPF16LE,
                vec![[0x00, 0xb4]; 4],
                vec![[0x00, 0x38]; 4]
            )
        );
    }

    #[test]
    fn a_gray_float_picture_keeps_its_linear_transfer() {
        let mut gray = filled(Pixel::GRAYF32LE, &[0.18f32.to_le_bytes()]);
        gray.set_color_transfer_characteristic(color::TransferCharacteristic::Linear);
        let out = Relayout::new().run(&gray, Reading::Stream).unwrap();
        assert_eq!(
            out.color_transfer_characteristic(),
            color::TransferCharacteristic::Linear
        );
    }

    #[test]
    fn a_display_doesnt_take_8_bit_4_4_4() {
        assert!(!displayable(Pixel::YUV444P));
    }

    #[test]
    fn a_4_4_4_picture_keeps_its_chroma_matrix_and_range() {
        let mut f = frame::Video::new(Pixel::YUV444P, 64, 64);
        f.set_color_space(color::Space::BT709);
        f.set_color_range(color::Range::MPEG);
        f.set_pts(Some(0));
        for (plane, v) in [(0, 100), (1, 90), (2, 170)] {
            f.data_mut(plane).fill(v);
        }
        let out = Relayout::new().run(&f, Reading::Stream).unwrap();
        let samples = |plane: usize, side: usize| -> Vec<u16> {
            let stride = out.stride(plane);
            (0..side)
                .flat_map(|y| {
                    out.data(plane)[y * stride..y * stride + side * 2]
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|&b| u16::from_le_bytes(b))
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        assert_eq!(out.format(), Pixel::YUV444P12LE);
        assert_eq!(samples(0, 64), vec![100 << 4; 64 * 64]);
        assert_eq!(samples(1, 64), vec![90 << 4; 64 * 64]);
        assert_eq!(samples(2, 64), vec![170 << 4; 64 * 64]);
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
