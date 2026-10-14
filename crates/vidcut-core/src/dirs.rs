//! Where vidcut keeps files outside projects, on each system.
//!
//! The XDG variables (`XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_STATE_HOME`)
//! are honored everywhere, so tests and scripts can move everything.
//! Without them: the XDG defaults under the home folder; on Windows the
//! known folders, `%APPDATA%` for settings and `%LOCALAPPDATA%` for caches
//! and state; on macOS the folders under `~/Library`, `Application Support`
//! for settings and state and `Caches` for caches.

use std::path::{Path, PathBuf};

/// The user's home folder (`HOME`; on Windows the profile folder).
pub fn home() -> PathBuf {
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// An XDG variable that is set.
fn xdg(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// A Windows known folder from its variable (`APPDATA`, `LOCALAPPDATA`), or
/// where it is by default under the profile.
#[cfg(windows)]
fn known(var: &str, default: &str) -> PathBuf {
    xdg(var).unwrap_or_else(|| home().join("AppData").join(default))
}

/// vidcut's folder in one of macOS's under `~/Library` (`Application
/// Support`, `Caches`).
#[cfg(target_os = "macos")]
fn library(folder: &str) -> PathBuf {
    home().join("Library").join(folder).join("vidcut")
}

/// Settings: `~/.config/vidcut` (`%APPDATA%\vidcut` on Windows,
/// `~/Library/Application Support/vidcut` on macOS): user presets and
/// preferences.
pub fn config() -> PathBuf {
    if let Some(dir) = xdg("XDG_CONFIG_HOME") {
        return dir.join("vidcut");
    }
    #[cfg(windows)]
    let dir = known("APPDATA", "Roaming").join("vidcut");
    #[cfg(target_os = "macos")]
    let dir = library("Application Support");
    #[cfg(not(any(windows, target_os = "macos")))]
    let dir = home().join(".config").join("vidcut");
    dir
}

/// What can be made again: `~/.cache/vidcut` (`%LOCALAPPDATA%\vidcut\cache`
/// on Windows, `~/Library/Caches/vidcut` on macOS).
pub fn cache() -> PathBuf {
    if let Some(dir) = xdg("XDG_CACHE_HOME") {
        return dir.join("vidcut");
    }
    #[cfg(windows)]
    let dir = known("LOCALAPPDATA", "Local").join("vidcut").join("cache");
    #[cfg(target_os = "macos")]
    let dir = library("Caches");
    #[cfg(not(any(windows, target_os = "macos")))]
    let dir = home().join(".cache").join("vidcut");
    dir
}

/// What outlives a run but isn't a setting: `~/.local/state/vidcut`
/// (`%LOCALAPPDATA%\vidcut\state` on Windows, `~/Library/Application
/// Support/vidcut/state` on macOS): undo histories.
pub fn state() -> PathBuf {
    if let Some(dir) = xdg("XDG_STATE_HOME") {
        return dir.join("vidcut");
    }
    #[cfg(windows)]
    let dir = known("LOCALAPPDATA", "Local").join("vidcut").join("state");
    #[cfg(target_os = "macos")]
    let dir = library("Application Support").join("state");
    #[cfg(not(any(windows, target_os = "macos")))]
    let dir = home().join(".local").join("state").join("vidcut");
    dir
}

/// `path` made absolute with its links resolved, as
/// [`std::fs::canonicalize`] makes it. On Windows that gives a verbatim
/// path (`\\?\C:\…`); it is kept in the usual form (`C:\…`, `\\server\…`)
/// where that names the same file, so paths compare and show as people
/// write them and project files stay readable.
pub fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    path.canonicalize().map(|p| plain(&p))
}

/// `path` without a verbatim prefix that it doesn't need.
fn plain(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use std::path::{Component, Prefix};
        let Some(Component::Prefix(prefix)) = path.components().next() else {
            return path.to_path_buf();
        };
        // In UTF-16 units, as Windows counts them: a name needn't be valid
        // Unicode.
        let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        // The path from unit `from` on, after `lead`.
        let rest = |from: usize, lead: &[u16]| {
            PathBuf::from(OsString::from_wide(&[lead, &wide[from..]].concat()))
        };
        let usual = match prefix.kind() {
            // `\\?\C:\…`
            Prefix::VerbatimDisk(_) => rest(4, &[]),
            // `\\?\UNC\server\…`
            Prefix::VerbatimUNC(..) => rest(8, &[0x5c, 0x5c]),
            _ => return path.to_path_buf(),
        };
        // Longer paths need the prefix (the usual form's limit is 260 units,
        // its end among them); names it allows that the usual form doesn't
        // (a trailing dot or space) are left as they are too.
        let fits = usual.as_os_str().encode_wide().count() < 260
            && !path.components().any(|c| {
                let last = c.as_os_str().encode_wide().last();
                matches!(c, Component::Normal(_)) && matches!(last, Some(0x2e | 0x20))
            });
        if fits { usual } else { path.to_path_buf() }
    }
    #[cfg(not(windows))]
    path.to_path_buf()
}
