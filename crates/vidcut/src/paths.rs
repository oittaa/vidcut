use std::path::PathBuf;

use vidcut_core::fingerprint::Origin;
use vidcut_core::model::Media;

pub fn config_dir() -> PathBuf {
    vidcut_core::dirs::config()
}

pub fn cache_dir() -> PathBuf {
    vidcut_core::dirs::cache()
}

pub fn media_origin(media: &Media) -> Origin {
    vidcut_core::fingerprint::origin(&media.path, media.size, media.mtime_ns)
}

pub fn media_cache(dir: &str, origin: &Origin, ext: &str) -> PathBuf {
    cache_dir().join(dir).join(format!("{}.{ext}", origin.name))
}

pub fn state_dir() -> PathBuf {
    vidcut_core::store::state_dir()
}
