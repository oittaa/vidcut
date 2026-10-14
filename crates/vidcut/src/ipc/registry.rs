//! Running GUIs announce themselves with `<pid>.json` (and listen on
//! `<pid>.sock`, on Windows a pipe named after it) in
//! `$XDG_RUNTIME_DIR/vidcut/instances/`. Entries of processes that are gone
//! are removed when listed.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A running GUI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instance {
    pub pid: u32,
    pub socket: PathBuf,
    /// The open project (canonical path), if any.
    pub project: Option<PathBuf>,
    pub version: String,
}

pub fn dir() -> PathBuf {
    super::runtime_dir().join("instances")
}

/// Where this process's GUI listens: `<pid>.sock` in [`dir`]. On Windows
/// the name also holds a number picked at random, as the pipe named after
/// it is in a namespace every user sees: nobody can take it first.
pub fn socket_path(pid: u32) -> PathBuf {
    #[cfg(windows)]
    let name = {
        use std::hash::{BuildHasher, RandomState};
        static SALT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        let salt = SALT.get_or_init(|| RandomState::new().hash_one(std::process::id()));
        format!("{pid}-{salt:016x}.sock")
    };
    #[cfg(not(windows))]
    let name = format!("{pid}.sock");
    dir().join(name)
}

fn entry_path(pid: u32) -> PathBuf {
    dir().join(format!("{pid}.json"))
}

/// Writes (or rewrites) this instance's entry atomically.
pub fn register(instance: &Instance) -> std::io::Result<()> {
    let dir = dir();
    super::private_dir(&dir)?;
    let path = entry_path(instance.pid);
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(instance).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)
}

/// Removes this instance's entry and socket.
pub fn unregister(pid: u32) {
    let _ = std::fs::remove_file(entry_path(pid));
    let _ = std::fs::remove_file(socket_path(pid));
}

/// Whether a process with this pid exists.
#[cfg(unix)]
pub(crate) fn alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 only checks for existence and permission.
    let r = unsafe { libc::kill(pid, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Whether a process with this pid is running.
#[cfg(windows)]
pub(crate) fn alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, GetLastError, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: opens the process only to read its exit code, and closes it.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            // Another user's process is there, but not ours to open.
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let mut code = 0u32;
        let read = GetExitCodeProcess(process, &mut code);
        CloseHandle(process);
        read != 0 && code == STILL_ACTIVE as u32
    }
}

fn read(path: &Path) -> Option<Instance> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Running GUIs, oldest pid first. Stale entries are cleaned up.
pub fn list() -> Vec<Instance> {
    let Ok(entries) = std::fs::read_dir(dir()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        match read(&path) {
            Some(i) if alive(i.pid) && super::transport::exists(&i.socket) => out.push(i),
            Some(i) if !alive(i.pid) => unregister(i.pid),
            _ => {}
        }
    }
    out.sort_by_key(|i| i.pid);
    out
}
