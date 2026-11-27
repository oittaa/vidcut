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

#[cfg(windows)]
fn replaced(path: &Path) -> bool {
    use std::path::Component;
    let mut names = path.components().filter_map(|c| match c {
        Component::Normal(name) => Some(name),
        _ => None,
    });
    names.next().is_some_and(|n| n == "$Extend") && names.next().is_some_and(|n| n == "$Deleted")
}

#[cfg(not(windows))]
fn replaced(_path: &Path) -> bool {
    false
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

pub fn on_network(path: &Path) -> bool {
    let Some(path) = path.ancestors().find(|p| p.exists()) else {
        return false;
    };
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        const CIFS: u32 = 0xFF53_4D42;
        const SMB2: u32 = 0xFE53_4D42;
        let Ok(name) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: an all-zero `statfs` is valid, and statfs writes only the one given.
        let mut s: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: `name` is a NUL-terminated path and `s` a live `statfs`.
        if unsafe { libc::statfs(name.as_ptr(), &mut s) } != 0 {
            return false;
        }
        let remote = [
            libc::NFS_SUPER_MAGIC as u32,
            libc::SMB_SUPER_MAGIC as u32,
            CIFS,
            SMB2,
        ];
        remote.contains(&(s.f_type as u32))
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(name) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: an all-zero `statfs` is valid, and statfs writes only the one given.
        let mut s: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: `name` is a NUL-terminated path and `s` a live `statfs`.
        if unsafe { libc::statfs(name.as_ptr(), &mut s) } != 0 {
            return false;
        }
        (s.f_flags & libc::MNT_LOCAL as u32) == 0
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumePathNameW};
        const DRIVE_REMOTE: u32 = 4;
        let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let mut root = vec![0u16; name.len() + 1];
        // SAFETY: `name` is a NUL-terminated path; `root` holds the count of u16s that is given.
        let ok = unsafe { GetVolumePathNameW(name.as_ptr(), root.as_mut_ptr(), root.len() as u32) };
        // SAFETY: on success, `root` holds a NUL-terminated path.
        ok != 0 && unsafe { GetDriveTypeW(root.as_ptr()) } == DRIVE_REMOTE
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = path;
        false
    }
}
