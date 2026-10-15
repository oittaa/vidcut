//! Where vidcut keeps files outside projects ([`vidcut_core::dirs`]: the XDG
//! base directories, Windows's known folders, macOS's `~/Library`).

use std::path::PathBuf;

use vidcut_core::fingerprint::Origin;
use vidcut_core::model::Media;

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

/// What the files made from `media`'s file as it is now say they were made
/// from: its path, size and modification time.
pub fn media_origin(media: &Media) -> Origin {
    vidcut_core::fingerprint::origin(&media.path, media.size, media.mtime_ns)
}

/// A file in `cache_dir()/dir` for what was derived from the file of
/// `origin` ([`media_origin`]), ending in `.ext`. It holds its origin
/// ([`Origin::stamp`]): one made from another file is made again.
pub fn media_cache(dir: &str, origin: &Origin, ext: &str) -> PathBuf {
    cache_dir().join(dir).join(format!("{}.{ext}", origin.name))
}

/// `~/.local/state/vidcut` (`%LOCALAPPDATA%\vidcut\state` on Windows,
/// `~/Library/Application Support/vidcut/state` on macOS): undo history,
/// recent projects.
pub fn state_dir() -> PathBuf {
    vidcut_core::store::state_dir()
}
