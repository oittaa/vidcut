use std::io::Cursor;

use ::exr::block::{self, reader::ChunksReader};
use ::exr::meta::attribute::{ChannelList, SampleType};
use ::exr::prelude::Vec2;

use crate::ffmpeg::{self, codec, ffi, format, packet::Mut, packet::Ref};

const RED: usize = 0;
const GREEN: usize = 1;
const BLUE: usize = 2;
const ALPHA: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Shape {
    luma: bool,
    alpha: bool,
    sample: SampleType,
}

impl Shape {
    fn all() -> impl Iterator<Item = Shape> {
        [SampleType::F16, SampleType::F32, SampleType::U32]
            .into_iter()
            .flat_map(|sample| {
                [(false, false), (false, true), (true, false), (true, true)].map(|(luma, alpha)| {
                    Shape {
                        luma,
                        alpha,
                        sample,
                    }
                })
            })
    }

    fn of(pixel: format::Pixel) -> Option<Shape> {
        Shape::all().find(|s| s.pixel() == pixel)
    }

    fn pixel(self) -> format::Pixel {
        use format::Pixel as P;
        match (self.sample, self.luma, self.alpha) {
            (SampleType::F16, false, false) => P::GBRPF16LE,
            (SampleType::F16, false, true) => P::GBRAPF16LE,
            (SampleType::F16, true, false) => P::GRAYF16LE,
            (SampleType::F16, true, true) => P::YAF16LE,
            (SampleType::F32, false, false) => P::GBRPF32LE,
            (SampleType::F32, false, true) => P::GBRAPF32LE,
            (SampleType::F32, true, false) => P::GRAYF32LE,
            (SampleType::F32, true, true) => P::YAF32LE,
            (SampleType::U32, false, false) => P::RGB48LE,
            (SampleType::U32, false, true) => P::RGBA64LE,
            (SampleType::U32, true, false) => P::GRAY16LE,
            (SampleType::U32, true, true) => P::YA16LE,
        }
    }

    fn order(self) -> &'static [usize] {
        match (self.luma, self.alpha, self.sample) {
            (true, false, _) => &[GREEN],
            (true, true, _) => &[GREEN, ALPHA],
            (false, false, SampleType::U32) => &[RED, GREEN, BLUE],
            (false, true, SampleType::U32) => &[RED, GREEN, BLUE, ALPHA],
            (false, false, _) => &[GREEN, BLUE, RED],
            (false, true, _) => &[GREEN, BLUE, RED, ALPHA],
        }
    }

    fn planar(self) -> bool {
        !self.luma && self.sample != SampleType::U32
    }

    fn bytes(self) -> usize {
        match self.sample {
            SampleType::F32 => 4,
            SampleType::F16 | SampleType::U32 => 2,
        }
    }

    fn frame_bytes(self, (w, h): (usize, usize)) -> usize {
        self.order().len() * w * h * self.bytes()
    }
}

fn role(letter: u8) -> Option<usize> {
    match letter {
        b'R' | b'X' | b'U' => Some(RED),
        b'G' | b'V' | b'Y' => Some(GREEN),
        b'B' | b'Z' | b'W' => Some(BLUE),
        b'A' => Some(ALPHA),
        _ => None,
    }
}

fn chosen(channels: &ChannelList) -> Option<([Option<usize>; 4], Shape)> {
    let mut slots = [None; 4];
    let mut letters = 0u32;
    let mut sample = None;
    for (i, channel) in channels.list.iter().enumerate() {
        let [letter] = channel.name.bytes() else {
            continue;
        };
        let letter = letter.to_ascii_uppercase();
        if letter.is_ascii_uppercase() {
            letters |= 1 << (letter - b'A');
        }
        let Some(r) = role(letter) else {
            continue;
        };
        if slots[r].is_some() {
            continue;
        }
        if channel.sampling != Vec2(1, 1) || sample.is_some_and(|s| s != channel.sample_type) {
            return None;
        }
        sample = Some(channel.sample_type);
        slots[r] = Some(i);
    }
    let has = |s: &[u8]| s.iter().all(|&l| letters & (1 << (l - b'A')) != 0);
    let any = |s: &[u8]| s.iter().any(|&l| letters & (1 << (l - b'A')) != 0);
    let luma = if has(b"RGB") || has(b"XYZ") || has(b"YUV") {
        false
    } else if has(b"Y") && !any(b"RGBUVXZ") {
        true
    } else {
        return None;
    };
    if !luma && slots[..ALPHA].iter().any(Option::is_none) {
        return None;
    }
    let shape = Shape {
        luma,
        alpha: slots[ALPHA].is_some(),
        sample: sample?,
    };
    Some((slots, shape))
}

fn decode(file: &[u8], shape: Shape, (w, h): (usize, usize), out: &mut [u8]) -> Result<(), String> {
    if out.len() != shape.frame_bytes((w, h)) {
        return Err(format!("{} bytes for a frame of {w}×{h}", out.len()));
    }
    let reader = block::read(Cursor::new(file), false).map_err(|e| e.to_string())?;
    let header = reader
        .meta_data()
        .headers
        .first()
        .ok_or_else(|| "no header".to_string())?
        .clone();
    if header.deep {
        return Err("deep data".into());
    }
    let (slots, found) =
        chosen(&header.channels).ok_or_else(|| "no R, G and B or Y channels".to_string())?;
    if found != shape {
        return Err(format!("{:?}, not {:?}", found.pixel(), shape.pixel()));
    }
    let display = header.shared_attributes.display_window;
    if display.size != Vec2(w, h) {
        return Err(format!(
            "a picture of {}×{}, not {w}×{h}",
            display.size.0, display.size.1
        ));
    }
    let origin = (
        i64::from(header.own_attributes.layer_position.0) - i64::from(display.position.0),
        i64::from(header.own_attributes.layer_position.1) - i64::from(display.position.1),
    );
    let order = shape.order();
    let mut component = vec![None; header.channels.list.len()];
    for (k, &r) in order.iter().enumerate() {
        if let Some(i) = slots[r] {
            component[i] = Some(k);
        }
    }
    let dw = shape.bytes();
    let (kstride, step) = match shape.planar() {
        true => (w * h * dw, dw),
        false => (dw, order.len() * dw),
    };
    let chunks = reader
        .filter_chunks(false, |_, _, b| b.layer == 0 && b.level == Vec2(0, 0))
        .map_err(|e| e.to_string())?;
    chunks
        .decompress_sequential(false, |_, block| {
            for line in block.lines(&header.channels) {
                let at = line.location;
                let Some(k) = component.get(at.channel).copied().flatten() else {
                    continue;
                };
                let y = origin.1 + at.position.1 as i64;
                if !(0..h as i64).contains(&y) {
                    continue;
                }
                let x0 = origin.0 + at.position.0 as i64;
                let first = (-x0).clamp(0, at.sample_count as i64) as usize;
                let last = (w as i64 - x0).clamp(first as i64, at.sample_count as i64) as usize;
                if first == last {
                    continue;
                }
                let start = k * kstride + (y as usize * w + (x0 + first as i64) as usize) * step;
                let dst = &mut out[start..];
                match shape.sample {
                    SampleType::F16 => {
                        let src = &line.value.as_chunks::<2>().0[first..last];
                        for (i, s) in src.iter().enumerate() {
                            dst[i * step..i * step + 2]
                                .copy_from_slice(&u16::from_ne_bytes(*s).to_le_bytes());
                        }
                    }
                    SampleType::F32 => {
                        let src = &line.value.as_chunks::<4>().0[first..last];
                        for (i, s) in src.iter().enumerate() {
                            dst[i * step..i * step + 4]
                                .copy_from_slice(&u32::from_ne_bytes(*s).to_le_bytes());
                        }
                    }
                    SampleType::U32 => {
                        let src = &line.value.as_chunks::<4>().0[first..last];
                        for (i, s) in src.iter().enumerate() {
                            let high = (u32::from_ne_bytes(*s) >> 16) as u16;
                            dst[i * step..i * step + 2].copy_from_slice(&high.to_le_bytes());
                        }
                    }
                }
            }
            Ok(())
        })
        .map_err(|e| e.to_string())
}

pub(crate) struct Planes {
    shape: Shape,
    size: (usize, usize),
}

impl Planes {
    pub(crate) fn take(ctx: &mut codec::context::Context) -> Option<Self> {
        // SAFETY: the context isn't opened yet.
        let c = unsafe { &mut *ctx.as_mut_ptr() };
        if c.codec_id != ffi::AVCodecID::AV_CODEC_ID_EXR {
            return None;
        }
        let shape = Shape::of(format::Pixel::from(c.pix_fmt))?;
        let size = (
            usize::try_from(c.width).ok()?,
            usize::try_from(c.height).ok()?,
        );
        c.codec_id = ffi::AVCodecID::AV_CODEC_ID_RAWVIDEO;
        c.codec_tag = 0;
        Some(Self { shape, size })
    }

    pub(crate) fn packet(&self, packet: &ffmpeg::Packet) -> Result<ffmpeg::Packet, ffmpeg::Error> {
        let file = packet.data().ok_or(ffmpeg::Error::InvalidData)?;
        let mut raw = ffmpeg::Packet::new(self.shape.frame_bytes(self.size));
        let out = raw.data_mut().ok_or(ffmpeg::Error::InvalidData)?;
        out.fill(0);
        decode(file, self.shape, self.size, out).map_err(|e| {
            crate::debug!(
                "decoding",
                "{}: an OpenEXR frame: {e}",
                super::thread_name()
            );
            ffmpeg::Error::InvalidData
        })?;
        // SAFETY: two packets of their own; FFmpeg copies the properties, not the data.
        let ret = unsafe { ffi::av_packet_copy_props(raw.as_mut_ptr(), packet.as_ptr()) };
        match ret {
            0.. => Ok(raw),
            _ => Err(ffmpeg::Error::from(ret)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::exr::meta::attribute::IntegerBounds;
    use ::exr::prelude::{
        AnyChannel, AnyChannels, Blocks, Compression, Encoding, FlatSamples, Image, Layer,
        LayerAttributes, LineOrder, SmallVec, WritableImage, f16,
    };

    fn file(
        channels: Vec<(&str, FlatSamples)>,
        size: (usize, usize),
        position: Vec2<i32>,
        display: (usize, usize),
    ) -> Vec<u8> {
        let list: SmallVec<[AnyChannel<FlatSamples>; 4]> = channels
            .into_iter()
            .map(|(name, sample_data)| AnyChannel {
                name: name.into(),
                sample_data,
                quantize_linearly: false,
                sampling: Vec2(1, 1),
            })
            .collect();
        let layer = Layer::new(
            size,
            LayerAttributes {
                layer_position: position,
                ..LayerAttributes::default()
            },
            Encoding {
                compression: Compression::PIZ,
                blocks: Blocks::ScanLines,
                line_order: LineOrder::Increasing,
            },
            AnyChannels::sort(list),
        );
        let mut image = Image::from_layer(layer);
        image.attributes.display_window =
            IntegerBounds::new(Vec2(0, 0), Vec2(display.0, display.1));
        let mut bytes = Vec::new();
        image.write().to_buffered(Cursor::new(&mut bytes)).unwrap();
        bytes
    }

    fn halves(v: &[f32]) -> FlatSamples {
        FlatSamples::F16(v.iter().map(|&x| f16::from_f32(x)).collect())
    }

    fn decoded(bytes: &[u8], pixel: format::Pixel, size: (usize, usize)) -> Vec<u8> {
        let shape = Shape::of(pixel).unwrap();
        let mut out = vec![0; shape.frame_bytes(size)];
        decode(bytes, shape, size, &mut out).unwrap();
        out
    }

    #[test]
    fn half_rgb_gives_green_blue_red_planes() {
        let bytes = file(
            vec![
                ("R", halves(&[1.0])),
                ("G", halves(&[2.0])),
                ("B", halves(&[0.5])),
            ],
            (1, 1),
            Vec2(0, 0),
            (1, 1),
        );
        assert_eq!(
            decoded(&bytes, format::Pixel::GBRPF16LE, (1, 1)),
            [0x00, 0x40, 0x00, 0x38, 0x00, 0x3c]
        );
    }

    #[test]
    fn float_piz_keeps_the_bits() {
        let bytes = file(
            vec![
                ("R", FlatSamples::F32(vec![0.1])),
                ("G", FlatSamples::F32(vec![-2.5])),
                ("B", FlatSamples::F32(vec![1e-30])),
            ],
            (1, 1),
            Vec2(0, 0),
            (1, 1),
        );
        assert_eq!(
            decoded(&bytes, format::Pixel::GBRPF32LE, (1, 1)),
            [
                0x00, 0x00, 0x20, 0xc0, 0x60, 0x42, 0xa2, 0x0d, 0xcd, 0xcc, 0xcc, 0x3d
            ]
        );
    }

    #[test]
    fn alpha_is_the_fourth_plane() {
        let bytes = file(
            vec![
                ("A", halves(&[0.5])),
                ("R", halves(&[0.0])),
                ("G", halves(&[0.0])),
                ("B", halves(&[0.0])),
            ],
            (1, 1),
            Vec2(0, 0),
            (1, 1),
        );
        assert_eq!(
            decoded(&bytes, format::Pixel::GBRAPF16LE, (1, 1)),
            [0, 0, 0, 0, 0, 0, 0x00, 0x38]
        );
    }

    #[test]
    fn u_v_and_w_beside_y_are_red_green_blue() {
        let bytes = file(
            vec![
                ("U", halves(&[1.0])),
                ("V", halves(&[2.0])),
                ("W", halves(&[0.5])),
                ("Y", halves(&[0.25])),
            ],
            (1, 1),
            Vec2(0, 0),
            (1, 1),
        );
        assert_eq!(
            decoded(&bytes, format::Pixel::GBRPF16LE, (1, 1)),
            [0x00, 0x40, 0x00, 0x38, 0x00, 0x3c]
        );
    }

    #[test]
    fn y_and_alpha_alone_interleave() {
        let bytes = file(
            vec![("A", halves(&[1.0, 0.5])), ("Y", halves(&[2.0, 0.0]))],
            (2, 1),
            Vec2(0, 0),
            (2, 1),
        );
        assert_eq!(
            decoded(&bytes, format::Pixel::YAF16LE, (2, 1)),
            [0x00, 0x40, 0x00, 0x3c, 0x00, 0x00, 0x00, 0x38]
        );
    }

    #[test]
    fn uint_keeps_the_high_16_bits() {
        let bytes = file(
            vec![
                ("R", FlatSamples::U32(vec![0x1234_5678])),
                ("G", FlatSamples::U32(vec![0xffff_0000])),
                ("B", FlatSamples::U32(vec![0x0000_ffff])),
            ],
            (1, 1),
            Vec2(0, 0),
            (1, 1),
        );
        assert_eq!(
            decoded(&bytes, format::Pixel::RGB48LE, (1, 1)),
            [0x34, 0x12, 0xff, 0xff, 0x00, 0x00]
        );
    }

    #[test]
    fn the_data_window_sits_in_the_picture() {
        let bytes = file(vec![("Y", halves(&[1.0]))], (1, 1), Vec2(1, 0), (2, 2));
        assert_eq!(
            decoded(&bytes, format::Pixel::GRAYF16LE, (2, 2)),
            [0x00, 0x00, 0x00, 0x3c, 0x00, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn a_layer_name_is_not_a_color() {
        let bytes = file(
            vec![
                ("diffuse.R", halves(&[0.0])),
                ("diffuse.G", halves(&[0.0])),
                ("diffuse.B", halves(&[0.0])),
            ],
            (1, 1),
            Vec2(0, 0),
            (1, 1),
        );
        let shape = Shape::of(format::Pixel::GBRPF16LE).unwrap();
        let mut out = vec![0; 6];
        assert_eq!(
            decode(&bytes, shape, (1, 1), &mut out),
            Err("no R, G and B or Y channels".to_string())
        );
    }

    #[test]
    fn another_sample_type_than_the_stream_is_refused() {
        let bytes = file(
            vec![("Y", FlatSamples::F32(vec![1.0]))],
            (1, 1),
            Vec2(0, 0),
            (1, 1),
        );
        let shape = Shape::of(format::Pixel::GRAYF16LE).unwrap();
        let mut out = vec![0; 2];
        assert_eq!(
            decode(&bytes, shape, (1, 1), &mut out),
            Err("GRAYF32LE, not GRAYF16LE".to_string())
        );
    }
}
