//! Pool thumbnails kept on disk (preference `thumbnails.disk_cache`), so a
//! project opens with its thumbnails shown instead of decoding every clip
//! again. One file per version of a media file (path, size and modification
//! time); above `thumbnails.cache_limit_mb` the least recently used go first.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use vidcut_av::thumbs::Image;
use vidcut_core::model::Media;

use crate::paths;

/// `~/.cache/vidcut/thumbs`.
pub fn dir() -> PathBuf {
    paths::cache_dir().join("thumbs")
}

/// The cache file of `media`'s filmstrip of `count` pictures fitted in
/// `size`: the layout is part of the name, so another layout never reads it.
fn file(media: &Media, count: usize, size: (u32, u32)) -> PathBuf {
    // A sequence's length is part of the name: the same first file with a
    // shorter count is a different strip (image2 would otherwise read on).
    let pictures = media
        .sequence
        .as_ref()
        .map(|s| format!("-{}", s.count))
        .unwrap_or_default();
    paths::media_cache(
        "thumbs",
        media,
        &format!("{count}-{}x{}{pictures}.thumbs", size.0, size.1),
    )
}

/// The cached filmstrip of `media`, if there is one; marks it as used.
pub fn load(media: &Media, count: usize, size: (u32, u32)) -> Option<(Vec<Image>, Vec<f64>)> {
    let path = file(media, count, size);
    let strip = vidcut_av::thumbs::from_bytes(&std::fs::read(&path).ok()?)?;
    // The modification time is the last use (opening for writing is the
    // portable way to set it).
    if let Ok(f) = std::fs::File::options().write(true).open(&path) {
        let _ = f.set_modified(SystemTime::now());
    }
    Some(strip)
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
    let path = file(media, count, size);
    let part = path.with_extension("part");
    let written = std::fs::create_dir_all(dir())
        .and_then(|()| std::fs::write(&part, &bytes))
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
