use std::path::Path;

use ::exr::meta::attribute::TimeCode;
use ::exr::prelude::{
    AnyChannel, AnyChannels, Blocks, Compression, Encoding, FlatSamples, Image, Layer,
    LayerAttributes, LineOrder, SmallVec, Vec2, WritableImage, f16,
};
use vidcut_core::export::{DWA_LEVEL, VideoSettings};
use vidcut_core::time::Rational;

use crate::error::{Error, Result};

pub(super) struct Writer {
    half: bool,
    compression: Compression,
    size: (usize, usize),
    names: &'static [&'static str],
    fps: (i32, u32),
}

const COLORS: [&str; 3] = ["G", "B", "R"];
const COLORS_ALPHA: [&str; 4] = ["G", "B", "R", "A"];

pub(super) fn chosen(profile: &str, level: Option<u32>) -> (bool, Compression) {
    let (kind, compression) = profile.split_once('_').unwrap_or((profile, "none"));
    let level = level.unwrap_or(DWA_LEVEL) as f32;
    let compression = match compression {
        "rle" => Compression::RLE,
        "zip1" => Compression::ZIP1,
        "zip16" => Compression::ZIP16,
        "piz" => Compression::PIZ,
        "dwaa" => Compression::DWAA(Some(level)),
        "dwab" => Compression::DWAB(Some(level)),
        _ => Compression::Uncompressed,
    };
    (kind == "half", compression)
}

impl Writer {
    pub(super) fn new(v: &VideoSettings, (width, height): (u32, u32), fps: Rational) -> Self {
        let (half, compression) =
            chosen(v.profile.as_deref().unwrap_or("half"), v.compression_level);
        Self {
            half,
            compression,
            size: (width as usize, height as usize),
            names: match v.alpha {
                true => &COLORS_ALPHA,
                false => &COLORS,
            },
            fps: (fps.num() as i32, fps.den() as u32),
        }
    }

    pub(super) fn write(&self, planes: &[u8], time: Option<u32>, path: &Path) -> Result<()> {
        let plane = self.size.0 * self.size.1 * 4;
        if planes.len() != plane * self.names.len() {
            return Err(Error::Invalid(format!(
                "an OpenEXR frame of {} bytes, not {}",
                planes.len(),
                plane * self.names.len()
            )));
        }
        let channels: SmallVec<[AnyChannel<FlatSamples>; 4]> = self
            .names
            .iter()
            .zip(planes.chunks_exact(plane))
            .map(|(&name, bytes)| {
                let values = bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes(*b));
                AnyChannel {
                    name: name.into(),
                    sample_data: match self.half {
                        true => FlatSamples::F16(values.map(f16::from_f32).collect()),
                        false => FlatSamples::F32(values.collect()),
                    },
                    quantize_linearly: false,
                    sampling: Vec2(1, 1),
                }
            })
            .collect();
        let layer = Layer::new(
            self.size,
            LayerAttributes {
                frames_per_second: Some(self.fps),
                ..LayerAttributes::default()
            },
            Encoding {
                compression: self.compression,
                blocks: Blocks::ScanLines,
                line_order: LineOrder::Increasing,
            },
            AnyChannels::sort(channels),
        );
        let mut image = Image::from_layer(layer);
        image.attributes.time_code = time.map(|t| TimeCode::from_tv60_time(t, 0));
        image
            .write()
            .to_file(path)
            .map_err(|e| Error::Invalid(format!("writing {}: {e}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dwab_without_a_level_takes_45() {
        assert_eq!(
            chosen("half_dwab", None),
            (true, Compression::DWAB(Some(45.0)))
        );
    }

    #[test]
    fn float_piz_is_float_with_piz() {
        assert_eq!(chosen("float_piz", None), (false, Compression::PIZ));
    }

    #[test]
    fn half_alone_has_no_compression() {
        assert_eq!(chosen("half", None), (true, Compression::Uncompressed));
    }

    #[test]
    fn the_time_code_keeps_the_second_frame_bit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frame.exr");
        let v = VideoSettings::new(
            vidcut_core::export::VideoCodec::Exr,
            vidcut_core::export::EncoderChoice::Cpu,
        );
        let writer = Writer::new(&v, (1, 1), Rational::new(60, 1).unwrap());
        writer.write(&[0; 12], Some(0x0100_8001), &path).unwrap();
        let meta = ::exr::meta::MetaData::read_from_file(&path, false).unwrap();
        let time = meta.headers[0].shared_attributes.time_code.unwrap();
        assert_eq!(time.pack_time_as_tv60_u32().unwrap(), 0x0100_8001);
    }
}
