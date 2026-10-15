//! Pool thumbnails kept on disk (preference `thumbnails.disk_cache`), so a
//! project opens with its thumbnails shown instead of decoding every clip
//! again. One file per version of a media file (path, size and modification
//! time), which holds that version's origin ([`Origin`]); above
//! `thumbnails.cache_limit_mb` the least recently used go first.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use vidcut_av::thumbs::Image;
use vidcut_core::fingerprint::Origin;
use vidcut_core::model::Media;

use crate::paths;

/// `~/.cache/vidcut/thumbs`.
pub fn dir() -> PathBuf {
    paths::cache_dir().join("thumbs")
}

/// The cache file of `media`'s filmstrip of `count` pictures fitted in
/// `size`, made from the file of `origin`: the layout is part of the name,
/// so another layout never reads it.
fn file(media: &Media, origin: &Origin, count: usize, size: (u32, u32)) -> PathBuf {
    // A sequence's length is part of the name: the same first file with a
    // shorter count is a different strip (image2 would otherwise read on).
    let pictures = media
        .sequence
        .as_ref()
        .map(|s| format!("-{}", s.count))
        .unwrap_or_default();
    paths::media_cache(
        "thumbs",
        origin,
        &format!("{count}-{}x{}{pictures}.thumbs", size.0, size.1),
    )
}

/// The cached filmstrip of `media`, if there is one; marks it as used.
pub fn load(media: &Media, count: usize, size: (u32, u32)) -> Option<(Vec<Image>, Vec<f64>)> {
    let origin = paths::media_origin(media);
    let path = file(media, &origin, count, size);
    let strip = vidcut_av::thumbs::from_bytes(origin.check(&std::fs::read(&path).ok()?)?)?;
    // The modification time is the last use (opening for writing is the
    // portable way to set it).
    if let Ok(f) = std::fs::File::options().write(true).open(&path) {
        let _ = f.set_modified(SystemTime::now());
    }
    Some(strip)
}

/// Source frames per file of timeline tiles.
pub const TILE_BLOCK: i64 = 64;

/// The cache file of the timeline tiles `height` pixels tall of the file of
/// `origin`, for the source frames from `block * TILE_BLOCK` to the next
/// block.
pub fn tile_block(origin: &Origin, height: u32, block: i64) -> PathBuf {
    paths::media_cache("thumbs", origin, &format!("t{height}-b{block}.thumbs"))
}

/// The tiles kept in the block file `path` of the file of `origin`, by
/// source frame; marks it as used.
pub fn load_tiles(path: &Path, origin: &Origin) -> Option<Vec<(i64, Image)>> {
    let (images, frames) =
        vidcut_av::thumbs::from_bytes(origin.check(&std::fs::read(path).ok()?)?)?;
    if let Ok(f) = std::fs::File::options().write(true).open(path) {
        let _ = f.set_modified(SystemTime::now());
    }
    Some(frames.into_iter().map(|f| f as i64).zip(images).collect())
}

/// Adds `tiles` of the file of `origin` to the block file `path`, with what
/// it holds of that file. Failing only costs decoding them again.
pub fn store_tiles(path: &Path, origin: &Origin, tiles: &[(i64, Image)]) {
    // Workers write blocks one at a time: two reading the same block before
    // either renames would each write it without the other's tiles.
    static WRITING: Mutex<()> = Mutex::new(());
    let _writing = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load_tiles(path, origin).unwrap_or_default();
    for (frame, image) in tiles {
        if !all.iter().any(|(f, _)| f == frame) {
            all.push((*frame, image.clone()));
        }
    }
    // One size per block: a picture of another (the media changed shape)
    // replaces what was there.
    if let Some((_, last)) = tiles.last() {
        all.retain(|(_, i)| (i.width, i.height) == (last.width, last.height));
    }
    all.sort_by_key(|(f, _)| *f);
    let (frames, images): (Vec<f64>, Vec<Image>) =
        all.into_iter().map(|(f, i)| (f as f64, i)).unzip();
    let Ok(bytes) = vidcut_av::thumbs::to_bytes(&images, &frames) else {
        return;
    };
    let part = path.with_extension(format!("part{}", std::process::id()));
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&part, origin.stamp(&bytes)))
        .and_then(|()| std::fs::rename(&part, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&part);
    }
}

/// Keeps `media`'s filmstrip, then trims the cache to `limit_mb`. Failing
/// to keep it only costs decoding it again next time.
pub fn store(
    media: &Media,
    count: usize,
    size: (u32, u32),
    strip: (&[Image], &[f64]),
    limit_mb: u64,
) {
    let Ok(bytes) = vidcut_av::thumbs::to_bytes(strip.0, strip.1) else {
        return;
    };
    let origin = paths::media_origin(media);
    let path = file(media, &origin, count, size);
    let part = path.with_extension("part");
    let written = std::fs::create_dir_all(dir())
        .and_then(|()| std::fs::write(&part, origin.stamp(&bytes)))
        .and_then(|()| std::fs::rename(&part, &path));
    if written.is_err() {
        let _ = std::fs::remove_file(&part);
        return;
    }
    evict(&dir(), limit_mb.saturating_mul(1 << 20));
}

/// Removes the least recently used thumbnails in `dir` until the rest take
/// at most `limit` bytes; returns how many went. Files being written stay.
pub fn evict(dir: &Path, limit: u64) -> usize {
    let mut files = cached(dir);
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= limit {
        return 0;
    }
    files.sort_by_key(|f| f.0);
    let mut removed = 0;
    for (_, len, path) in files {
        if total <= limit {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
            removed += 1;
        }
    }
    removed
}

/// How many thumbnail files the cache holds and their total size in bytes.
pub fn usage() -> (usize, u64) {
    let files = cached(&dir());
    (files.len(), files.iter().map(|f| f.1).sum())
}

/// Empties the cache.
pub fn clear() -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The finished cache files in `dir`: last use, size, path.
fn cached(dir: &Path) -> Vec<(SystemTime, u64, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "thumbs"))
        .filter_map(|e| {
            let meta = e.metadata().ok().filter(|m| m.is_file())?;
            Some((meta.modified().ok()?, meta.len(), e.path()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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

    #[test]
    fn eviction_drops_the_least_recently_used_first() {
        let tmp = tempfile::Builder::new()
            .prefix("vidcut-thumbcache-")
            .tempdir()
            .unwrap();
        let dir = tmp.path();
        let now = SystemTime::now();
        // a used 30 s ago, b 20 s ago, c 10 s ago; 1000 bytes each.
        for (name, age) in [("a", 30), ("b", 20), ("c", 10)] {
            let path = dir.join(format!("{name}.thumbs"));
            std::fs::write(&path, [0u8; 1000]).unwrap();
            let f = std::fs::File::options().write(true).open(&path).unwrap();
            f.set_modified(now - Duration::from_secs(age)).unwrap();
        }
        std::fs::write(dir.join("d.part"), [0u8; 5000]).unwrap();
        assert_eq!(evict(dir, 3000), 0, "within the limit");
        assert_eq!(evict(dir, 2500), 1);
        assert!(!dir.join("a.thumbs").exists(), "the oldest went");
        assert!(dir.join("b.thumbs").exists() && dir.join("c.thumbs").exists());
        assert_eq!(evict(dir, 0), 2);
        assert!(dir.join("d.part").exists(), "a file being written stays");
    }
}
