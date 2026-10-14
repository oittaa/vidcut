//! Where vidcut keeps files outside projects ([`vidcut_core::dirs`]: the XDG
//! base directories, Windows's known folders, macOS's `~/Library`).

use std::path::PathBuf;

/// `~/.config/vidcut` (`%APPDATA%\vidcut` on Windows, `~/Library/Application
/// Support/vidcut` on macOS): user presets and preferences.
pub fn config_dir() -> PathBuf {
    vidcut_core::dirs::config()
}

/// `~/.cache/vidcut` (`%LOCALAPPDATA%\vidcut\cache` on Windows,
/// `~/Library/Caches/vidcut` on macOS): encoder
/// detection, thumbnails, waveforms, the GPU converter's shaders and
/// proxies written when no project folder is known.
pub fn cache_dir() -> PathBuf {
    vidcut_core::dirs::cache()
}

/// A file in `cache_dir()/dir` for what was derived from `media`'s file as
/// it is now (path, size and modification time), ending in `.ext`.
pub fn media_cache(dir: &str, media: &vidcut_core::model::Media, ext: &str) -> PathBuf {
    cache_dir().join(dir).join(format!(
        "{:016x}-{}-{}.{ext}",
        vidcut_core::hash64(media.path.as_os_str().as_encoded_bytes()),
        media.size,
        media.mtime_ns
    ))
}

/// `~/.local/state/vidcut` (`%LOCALAPPDATA%\vidcut\state` on Windows,
/// `~/Library/Application Support/vidcut/state` on macOS): undo history,
/// recent projects.
pub fn state_dir() -> PathBuf {
    vidcut_core::store::state_dir()
}
