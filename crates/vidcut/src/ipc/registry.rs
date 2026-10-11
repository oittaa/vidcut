//! Running GUIs announce themselves with `<pid>.json` (and listen on
//! `<pid>.sock`) in `$XDG_RUNTIME_DIR/vidcut/instances/`. Entries of processes
//! that are gone are removed when listed.

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

pub fn socket_path(pid: u32) -> PathBuf {
    dir().join(format!("{pid}.sock"))
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
fn alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 only checks for existence and permission.
    let r = unsafe { libc::kill(pid, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
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
            Some(i) if alive(i.pid) && i.socket.exists() => out.push(i),
            Some(i) if !alive(i.pid) => unregister(i.pid),
            _ => {}
        }
    }
    out.sort_by_key(|i| i.pid);
    out
}
