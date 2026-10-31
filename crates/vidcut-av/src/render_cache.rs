use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use vidcut_core::render::RenderPlan;
use vidcut_core::render_cache::{FILE, KEYS, Key, name_of, write_keys};

use crate::error::{Error, Result};
use crate::ffmpeg;
use crate::proxy::{Failed, ProxyEncoder, encode_plan};

const LONG_GOP: [&str; 8] = [
    "h264",
    "hevc",
    "av1",
    "vp9",
    "vp8",
    "mpeg2video",
    "mpeg4",
    "jpeg2000",
];

pub fn slow(gpu: Option<&crate::hw::GpuDecoder>, path: &Path, codec: &str) -> bool {
    let Some(gpu) = gpu else {
        return false;
    };
    if !LONG_GOP.contains(&codec) {
        return false;
    }
    crate::init();
    let Some(id) = ffmpeg::codec::decoder::find_by_name(codec).map(|c| c.id()) else {
        return false;
    };
    let input = crate::export::video::input_of(path, None);
    !crate::decode::gpu_takes(id, gpu, || crate::decode::h264_open_gop(&input))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub path: PathBuf,
    pub frames: u64,
    pub encoder: String,
}

pub fn write(
    (plan, clip): (&RenderPlan, bool),
    keys: &[Key],
    encoders: &[(ProxyEncoder, bool)],
    decoder: Option<&crate::hw::GpuDecoder>,
    dir: &Path,
    cancel: &AtomicBool,
) -> Result<Written> {
    crate::init();
    if encoders.is_empty() {
        return Err(Error::Invalid(
            "no encoder on this computer makes the render cache".into(),
        ));
    }
    if keys.len() as i64 != plan.frames {
        return Err(Error::Invalid(format!(
            "{} names for {} frames of the render cache",
            keys.len(),
            plan.frames
        )));
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| Error::Invalid(format!("creating {}: {e}", dir.display())))?;
    let dest = dir.join(format!("{}.{FILE}", name_of(keys)));
    let (part, file) = vidcut_core::temp::file(&dest)
        .map_err(|e| Error::Invalid(format!("creating {}: {e}", dest.display())))?;
    drop(file);
    let mut errors = Vec::new();
    for (enc, high) in encoders {
        match encode_plan((plan, clip), (enc, *high), part.path(), decoder, cancel) {
            Ok(frames) => {
                part.keep(&dest)
                    .map_err(|e| Error::Invalid(format!("replacing {}: {e}", dest.display())))?;
                write_keys(&dest, keys).map_err(|e| {
                    let _ = std::fs::remove_file(&dest);
                    Error::Invalid(format!("writing the names of {}: {e}", dest.display()))
                })?;
                if !vidcut_core::disk_cache::added(&dest) {
                    let _ = std::fs::remove_file(dest.with_extension(KEYS));
                    return Err(Error::Invalid(
                        "the disk cache has no room for the render cache".into(),
                    ));
                }
                let encoder = match high {
                    true => enc.high_label(),
                    false => enc.label(),
                };
                return Ok(Written {
                    path: dest,
                    frames,
                    encoder,
                });
            }
            Err(Failed::Encoding(e)) => return Err(e),
            Err(Failed::Opening(e)) => errors.push(e),
        }
    }
    Err(errors.remove(0))
}
