use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError, TryLockError};
use std::time::SystemTime;

use crate::fingerprint::Origin;
use crate::preferences::{Cache, CacheLimit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Thumbs,
    Waveforms,
    Analysis,
    Shaders,
    DecoderTests,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Thumbs,
        Kind::Waveforms,
        Kind::Analysis,
        Kind::Shaders,
        Kind::DecoderTests,
    ];

    pub fn folder(self) -> &'static str {
        match self {
            Kind::Thumbs => "thumbs",
            Kind::Waveforms => "waveforms",
            Kind::Analysis => "analysis",
            Kind::Shaders => "shaders",
            Kind::DecoderTests => "decoder-tests",
        }
    }

    pub fn dir(self) -> PathBuf {
        crate::dirs::cache().join(self.folder())
    }

    pub fn file(self, origin: &Origin, ext: &str) -> PathBuf {
        self.dir().join(format!("{}.{ext}", origin.name))
    }

    fn keeps(self, name: &str) -> bool {
        self == Kind::Analysis
            && Path::new(name)
                .extension()
                .is_some_and(|e| e.to_string_lossy().starts_with("words"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disk {
    pub size: u64,
    pub free: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    pub cache: Cache,
    pub disk: Option<Disk>,
    pub proxies: PathBuf,
}

pub const KEEP_FREE_PERCENT: u64 = 20;

impl Policy {
    pub fn chosen(&self) -> Option<u64> {
        match self.cache.limit {
            CacheLimit::Percent => self
                .disk
                .map(|d| d.size / 100 * u64::from(self.cache.percent)),
            CacheLimit::Gb => self.cache.gb.map(|gb| gb.saturating_mul(1 << 30)),
        }
    }

    pub fn limit(&self, held: u64) -> Option<u64> {
        let chosen = self.chosen()?;
        Some(match self.disk {
            Some(d) => chosen.min(
                held.saturating_add(d.free)
                    .saturating_sub(d.size / 100 * KEEP_FREE_PERCENT),
            ),
            None => chosen,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub files: usize,
    pub bytes: u64,
    pub chosen: Option<u64>,
    pub limit: Option<u64>,
}

static POLICY: OnceLock<fn() -> Policy> = OnceLock::new();
static AGAIN: AtomicBool = AtomicBool::new(false);
static EVICTING: Mutex<()> = Mutex::new(());

pub fn governed_by(policy: fn() -> Policy) {
    let _ = POLICY.set(policy);
}

pub fn read(path: &Path) -> Option<Vec<u8>> {
    let bytes = std::fs::read(path).ok()?;
    used(path);
    Some(bytes)
}

pub fn used(path: &Path) {
    if !path.starts_with(crate::dirs::cache()) {
        return;
    }
    if let Ok(f) = std::fs::File::options().write(true).open(path) {
        let _ = f.set_modified(SystemTime::now());
    }
}

pub fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static WRITTEN: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let n = WRITTEN.fetch_add(1, Ordering::Relaxed);
    let part = path.with_file_name(format!(".{name}.{}.{n}.part", std::process::id()));
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&part, bytes))
        .and_then(|()| match stays(path, bytes.len() as u64) {
            true => std::fs::rename(&part, path),
            false => Err(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "the disk cache has no room for it",
            )),
        });
    if written.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    written
}

pub fn added(path: &Path) -> bool {
    used(path);
    let len = std::fs::metadata(path).map_or(0, |m| m.len());
    let kept = stays(path, len);
    if !kept {
        let _ = std::fs::remove_file(path);
    }
    kept
}

pub fn room(path: &Path) -> Option<u64> {
    let policy = POLICY.get()?();
    if !counted(&policy, path) {
        return None;
    }
    let found = entries(&policy);
    policy.limit(found.iter().filter(|e| e.path != path).map(|e| e.len).sum())
}

pub fn evict() {
    let Some(policy) = POLICY.get() else {
        return;
    };
    AGAIN.store(true, Ordering::Release);
    loop {
        let one = match EVICTING.try_lock() {
            Ok(one) => one,
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
            Err(TryLockError::WouldBlock) => return,
        };
        while AGAIN.swap(false, Ordering::AcqRel) {
            pass(&policy(), None);
        }
        drop(one);
        if !AGAIN.load(Ordering::Acquire) {
            return;
        }
    }
}

fn stays(path: &Path, len: u64) -> bool {
    let Some(policy) = POLICY.get().map(|p| p()) else {
        return true;
    };
    if !counted(&policy, path) {
        evict();
        return true;
    }
    let one = EVICTING.lock().unwrap_or_else(PoisonError::into_inner);
    AGAIN.store(false, Ordering::Release);
    let fits = pass(&policy, Some((path, len)));
    drop(one);
    if AGAIN.load(Ordering::Acquire) {
        evict();
    }
    fits
}

fn pass(policy: &Policy, new: Option<(&Path, u64)>) -> bool {
    let (path, len) = new.unwrap_or((Path::new(""), 0));
    let mut found = entries(policy);
    found.retain(|e| e.path != path);
    let held = found.iter().map(|e| e.len).sum::<u64>() + len;
    let Some(limit) = policy.limit(held) else {
        return true;
    };
    let fits = len <= limit;
    shrink(found, if fits { limit - len } else { limit });
    fits
}

fn counted(policy: &Policy, path: &Path) -> bool {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return false;
    };
    let name = name.to_string_lossy();
    Kind::ALL.iter().any(|k| dir == k.dir() && !k.keeps(&name))
        || (proxies(policy) == Some(dir) && name.ends_with(".mkv"))
}

fn proxies(policy: &Policy) -> Option<&Path> {
    let dir = &policy.proxies;
    (dir.starts_with(crate::dirs::cache()) && !Kind::ALL.iter().any(|k| *dir == k.dir()))
        .then_some(dir.as_path())
}

pub fn usage(policy: &Policy) -> Usage {
    let found = entries(policy);
    let bytes = found.iter().map(|e| e.len).sum();
    Usage {
        files: found.len(),
        bytes,
        chosen: policy.chosen(),
        limit: policy.limit(bytes),
    }
}

struct Entry {
    used: SystemTime,
    len: u64,
    path: PathBuf,
}

fn entries(policy: &Policy) -> Vec<Entry> {
    let mut found = Vec::new();
    for kind in Kind::ALL {
        listed(&kind.dir(), &|name| !kind.keeps(name), &mut found);
    }
    if let Some(dir) = proxies(policy) {
        listed(dir, &|name| name.ends_with(".mkv"), &mut found);
    }
    found
}

fn listed(dir: &Path, counts: &dyn Fn(&str) -> bool, found: &mut Vec<Entry>) {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return;
    };
    for e in listing.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.split('.').any(|s| s == "part") || !counts(&name) {
            continue;
        }
        let Ok(meta) = e.metadata() else {
            continue;
        };
        if let (true, Ok(used)) = (meta.is_file(), meta.modified()) {
            found.push(Entry {
                used,
                len: meta.len(),
                path: e.path(),
            });
        }
    }
}

fn shrink(mut entries: Vec<Entry>, limit: u64) -> usize {
    let mut total: u64 = entries.iter().map(|e| e.len).sum();
    if total <= limit {
        return 0;
    }
    entries.sort_by_key(|e| e.used);
    let mut removed = 0;
    for e in entries {
        if total <= limit {
            break;
        }
        if std::fs::remove_file(&e.path).is_ok() {
            total -= e.len;
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn eviction_drops_the_least_recently_used_first() {
        let tmp = tempfile::Builder::new()
            .prefix("vidcut-disk-cache-")
            .tempdir()
            .unwrap();
        let dir = tmp.path();
        let now = SystemTime::now();
        for (name, age) in [("a", 30), ("b", 20), ("c", 10)] {
            let path = dir.join(format!("{name}.thumbs"));
            std::fs::write(&path, [0u8; 1000]).unwrap();
            let f = std::fs::File::options().write(true).open(&path).unwrap();
            f.set_modified(now - Duration::from_secs(age)).unwrap();
        }
        std::fs::write(dir.join(".d.thumbs.1.0.part"), [0u8; 5000]).unwrap();
        let evict = |limit| {
            let mut found = Vec::new();
            listed(dir, &|_| true, &mut found);
            shrink(found, limit)
        };
        assert_eq!(evict(3000), 0, "within the limit");
        assert_eq!(evict(2500), 1);
        assert!(!dir.join("a.thumbs").exists(), "the oldest went");
        assert!(dir.join("b.thumbs").exists() && dir.join("c.thumbs").exists());
        assert_eq!(evict(0), 2);
        assert!(
            dir.join(".d.thumbs.1.0.part").exists(),
            "a file being written stays"
        );
    }
}
