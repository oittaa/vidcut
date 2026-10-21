use std::path::{Path, PathBuf};
use std::sync::Mutex;

use vidcut_av::thumbs::Image;
use vidcut_core::disk_cache::{self, Kind};
use vidcut_core::fingerprint::Origin;
use vidcut_core::model::ColorSpace;
use vidcut_core::model::Media;

use crate::paths;

pub fn dir() -> PathBuf {
    Kind::Thumbs.dir()
}

fn file(media: &Media, origin: &Origin, count: usize, size: (u32, u32)) -> PathBuf {
    let pictures = media
        .sequence
        .as_ref()
        .map(|s| format!("-{}", s.count))
        .unwrap_or_default();
    let space = read_as(media.color_space);
    Kind::Thumbs.file(
        origin,
        &format!("{count}-{}x{}{pictures}{space}.thumbs", size.0, size.1),
    )
}

pub fn load(media: &Media, count: usize, size: (u32, u32)) -> Option<(Vec<Image>, Vec<f64>)> {
    let origin = paths::media_origin(media);
    let path = file(media, &origin, count, size);
    vidcut_av::thumbs::from_bytes(origin.check(&disk_cache::read(&path)?)?)
}

pub const TILE_BLOCK: i64 = 64;

pub fn tile_block(origin: &Origin, height: u32, block: i64, space: ColorSpace) -> PathBuf {
    let space = read_as(space);
    Kind::Thumbs.file(origin, &format!("t{height}-b{block}{space}.thumbs"))
}

fn read_as(space: ColorSpace) -> String {
    match space {
        ColorSpace::Tagged => String::new(),
        s => format!("-{}", s.key()),
    }
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

pub fn store(media: &Media, count: usize, size: (u32, u32), strip: (&[Image], &[f64])) {
    let Ok(bytes) = vidcut_av::thumbs::to_bytes(strip.0, strip.1) else {
        return;
    };
    let origin = paths::media_origin(media);
    let _ = disk_cache::write(&file(media, &origin, count, size), &origin.stamp(&bytes));
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
