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

/// `~/.cache/vidcut`: encoder detection, preview fillers and remuxes.
pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache")
}

/// `~/.local/state/vidcut`: undo history, recent projects.
pub fn state_dir() -> PathBuf {
    vidcut_core::store::state_dir()
}
