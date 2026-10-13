//! Where vidcut keeps files outside projects, on each system.
//!
//! The XDG variables (`XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_STATE_HOME`)
//! are honored everywhere, so tests and scripts can move everything.
//! Without them: the XDG defaults under the home folder, and on Windows the
//! known folders, `%APPDATA%` for settings and `%LOCALAPPDATA%` for caches
//! and state.

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

/// Settings: `~/.config/vidcut` (`%APPDATA%\vidcut` on Windows): user
/// presets and preferences.
pub fn config() -> PathBuf {
    if let Some(dir) = xdg("XDG_CONFIG_HOME") {
        return dir.join("vidcut");
    }
    #[cfg(windows)]
    let dir = known("APPDATA", "Roaming").join("vidcut");
    #[cfg(not(windows))]
    let dir = home().join(".config").join("vidcut");
    dir
}

/// What can be made again: `~/.cache/vidcut` (`%LOCALAPPDATA%\vidcut\cache`
/// on Windows).
pub fn cache() -> PathBuf {
    if let Some(dir) = xdg("XDG_CACHE_HOME") {
        return dir.join("vidcut");
    }
    #[cfg(windows)]
    let dir = known("LOCALAPPDATA", "Local").join("vidcut").join("cache");
    #[cfg(not(windows))]
    let dir = home().join(".cache").join("vidcut");
    dir
}

/// What outlives a run but isn't a setting: `~/.local/state/vidcut`
/// (`%LOCALAPPDATA%\vidcut\state` on Windows): undo histories.
pub fn state() -> PathBuf {
    if let Some(dir) = xdg("XDG_STATE_HOME") {
        return dir.join("vidcut");
    }
    #[cfg(windows)]
    let dir = known("LOCALAPPDATA", "Local").join("vidcut").join("state");
    #[cfg(not(windows))]
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
        use std::path::{Component, Prefix};
        let Some(Component::Prefix(prefix)) = path.components().next() else {
            return path.to_path_buf();
        };
        let text = path.as_os_str().to_string_lossy();
        // Longer paths need the prefix; names it allows that the usual form
        // doesn't (a trailing dot or space) are left as they are too.
        let usual = text.len() < 260
            && !path.components().any(|c| {
                let s = c.as_os_str().to_string_lossy();
                matches!(c, Component::Normal(_)) && (s.ends_with('.') || s.ends_with(' '))
            });
        match prefix.kind() {
            Prefix::VerbatimDisk(_) if usual => PathBuf::from(&text[4..]),
            Prefix::VerbatimUNC(..) if usual => PathBuf::from(format!(r"\\{}", &text[8..])),
            _ => path.to_path_buf(),
        }
    }
    #[cfg(not(windows))]
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_paths_are_absolute() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.vidcut");
        std::fs::write(&file, "").unwrap();
        let found = canonical(&file).unwrap();
        assert!(found.is_absolute());
        assert_eq!(found.file_name(), file.file_name());
        assert_eq!(canonical(&found).unwrap(), found);
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_lose_a_verbatim_prefix_they_dont_need() {
        assert_eq!(
            plain(Path::new(r"\\?\C:\a\b.mp4")),
            Path::new(r"C:\a\b.mp4")
        );
        assert_eq!(
            plain(Path::new(r"\\?\UNC\server\share\b.mp4")),
            Path::new(r"\\server\share\b.mp4")
        );
        assert_eq!(plain(Path::new(r"C:\a")), Path::new(r"C:\a"));
        // A name the usual form can't hold keeps the prefix.
        assert_eq!(plain(Path::new(r"\\?\C:\a.")), Path::new(r"\\?\C:\a."));
        let long = format!(r"\\?\C:\{}", "x".repeat(300));
        assert_eq!(plain(Path::new(&long)), Path::new(&long));
    }
}
