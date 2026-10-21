use std::path::{Path, PathBuf};

pub fn home() -> PathBuf {
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn xdg(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

#[cfg(windows)]
fn known(var: &str, default: &str) -> PathBuf {
    xdg(var).unwrap_or_else(|| home().join("AppData").join(default))
}

#[cfg(target_os = "macos")]
fn library(folder: &str) -> PathBuf {
    home().join("Library").join(folder).join("vidcut")
}

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

pub fn models() -> PathBuf {
    cache().join("models")
}

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

pub fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    for _ in 0..100 {
        let p = path.canonicalize()?;
        if !replaced(&p) {
            return Ok(plain(&p));
        }
    }
    Err(std::io::Error::other(format!(
        "{} is replaced each time it is opened",
        path.display()
    )))
}

fn replaced(path: &Path) -> bool {
    use std::path::Component;
    let mut names = path.components().filter_map(|c| match c {
        Component::Normal(name) => Some(name),
        _ => None,
    });
    cfg!(windows)
        && names.next().is_some_and(|n| n == "$Extend")
        && names.next().is_some_and(|n| n == "$Deleted")
}

fn plain(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use std::path::{Component, Prefix};
        let Some(Component::Prefix(prefix)) = path.components().next() else {
            return path.to_path_buf();
        };
        let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        let rest = |from: usize, lead: &[u16]| {
            PathBuf::from(OsString::from_wide(&[lead, &wide[from..]].concat()))
        };
        let usual = match prefix.kind() {
            Prefix::VerbatimDisk(_) => rest(4, &[]),
            Prefix::VerbatimUNC(..) => rest(8, &[0x5c, 0x5c]),
            _ => return path.to_path_buf(),
        };
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
