use std::path::{Path, PathBuf};
use std::sync::Mutex;

use vidcut_av::thumbs::Image;
use vidcut_core::disk_cache::{self, Kind};
use vidcut_core::fingerprint::Origin;
use vidcut_core::model::{AlphaMode, ColorSpace, Media};

pub const THUMBS: usize = 24;

pub const THUMB_SIZE: (u32, u32) = (192, 108);

pub fn dir() -> PathBuf {
    Kind::Thumbs.dir()
}

pub fn thumbs_file(media: &Media, origin: &Origin) -> PathBuf {
    let pictures = media
        .sequence
        .as_ref()
        .map(|s| format!("-{}", s.count))
        .unwrap_or_default();
    let look = look_of(media);
    let (w, h) = THUMB_SIZE;
    Kind::Thumbs.file(origin, &format!("s{THUMBS}-{w}x{h}{pictures}{look}.thumbs"))
}

pub fn load_thumbs(path: &Path, origin: &Origin) -> Option<(Vec<Image>, Vec<f64>)> {
    vidcut_av::thumbs::from_bytes(origin.check(&disk_cache::read(path)?)?)
}

pub fn has_thumbs(path: &Path) -> bool {
    let there = path.is_file();
    if there {
        disk_cache::used(path);
    }
    there
}

pub fn store_thumbs(path: &Path, origin: &Origin, images: &[Image], times: &[f64]) -> bool {
    vidcut_av::thumbs::to_bytes(images, times)
        .is_ok_and(|bytes| disk_cache::write(path, &origin.stamp(&bytes)).is_ok())
}

pub const TILE_BLOCK: i64 = 64;

pub fn tile_block(origin: &Origin, height: u32, block: i64, media: &Media) -> PathBuf {
    let look = look_of(media);
    Kind::Thumbs.file(origin, &format!("t{height}-b{block}{look}.thumbs"))
}

pub fn drawn_alpha(media: &Media) -> AlphaMode {
    match &media.info.video {
        Some(v) if v.alpha => media.alpha_mode,
        _ => AlphaMode::Straight,
    }
}

fn look_of(media: &Media) -> String {
    let space = match media.color_space {
        ColorSpace::Tagged => String::new(),
        s => format!("-{}", s.key()),
    };
    let alpha = match drawn_alpha(media) {
        AlphaMode::Straight => String::new(),
        a => format!("-alpha-{}", a.title().to_lowercase()),
    };
    format!("{space}{alpha}")
}

pub fn load_tiles(path: &Path, origin: &Origin) -> Option<Vec<(i64, Image)>> {
    let (images, frames) = vidcut_av::thumbs::from_bytes(origin.check(&disk_cache::read(path)?)?)?;
    Some(frames.into_iter().map(|f| f as i64).zip(images).collect())
}

pub fn store_tiles(path: &Path, origin: &Origin, tiles: &[(i64, Image)]) {
    static WRITING: Mutex<()> = Mutex::new(());
    let _writing = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load_tiles(path, origin).unwrap_or_default();
    for (frame, image) in tiles {
        if !all.iter().any(|(f, _)| f == frame) {
            all.push((*frame, image.clone()));
        }
    }
    if let Some((_, last)) = tiles.last() {
        all.retain(|(_, i)| (i.width, i.height) == (last.width, last.height));
    }
    all.sort_by_key(|(f, _)| *f);
    let (frames, images): (Vec<f64>, Vec<Image>) =
        all.into_iter().map(|(f, i)| (f as f64, i)).unzip();
    if let Ok(bytes) = vidcut_av::thumbs::to_bytes(&images, &frames) {
        let _ = disk_cache::write(path, &origin.stamp(&bytes));
    }
}

pub fn clear() -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_are_read_only_for_the_file_they_were_made_from() {
        let tmp = tempfile::Builder::new()
            .prefix("vidcut-thumbcache-")
            .tempdir()
            .unwrap();
        let path = tmp.path().join("block.thumbs");
        let gray = Image {
            width: 16,
            height: 16,
            rgba: vec![128; 16 * 16 * 4],
        };
        let a = vidcut_core::fingerprint::origin(Path::new("/media/a.mov"), 1000, 1);
        let b = vidcut_core::fingerprint::origin(Path::new("/media/b.mov"), 1000, 1);
        store_tiles(&path, &a, &[(0, gray)]);
        assert_eq!(load_tiles(&path, &a).map(|tiles| tiles.len()), Some(1));
        assert!(load_tiles(&path, &b).is_none());
    }
}
