//! Where vidcut keeps files outside projects (XDG base directories).

use std::path::PathBuf;

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(fallback))
        .join("vidcut")
}

/// `~/.config/vidcut`: user presets.
pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
}

/// `~/.cache/vidcut`: encoder detection, preview fillers, remuxes, the GPU
/// converter's shaders and proxies written when no project folder is known.
pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache")
}

/// A file in `cache_dir()/dir` for what was derived from `media`'s file as
/// it is now (path, size and modification time), ending in `.ext`.
pub fn media_cache(dir: &str, media: &vidcut_core::model::Media, ext: &str) -> PathBuf {
    cache_dir().join(dir).join(format!(
        "{:016x}-{}-{}.{ext}",
        fnv1a(media.path.as_os_str().as_encoded_bytes()),
        media.size,
        media.mtime_ns
    ))
}

/// FNV-1a: stable 64-bit hash for cache file names.
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// `~/.local/state/vidcut`: undo history, recent projects.
pub fn state_dir() -> PathBuf {
    vidcut_core::store::state_dir()
}
