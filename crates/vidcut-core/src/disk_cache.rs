use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime};

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
    pub proxies: PathBuf,
}

pub const KEEP_FREE_PERCENT: u64 = 20;

pub const RESCAN_EVERY: Duration = Duration::from_secs(30);

pub const MARK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

impl Policy {
    pub fn chosen(&self, disk: Option<Disk>) -> Option<u64> {
        match self.cache.limit {
            CacheLimit::Percent => disk.map(|d| d.size / 100 * u64::from(self.cache.percent)),
            CacheLimit::Gb => self.cache.gb.map(|gb| gb.saturating_mul(1 << 30)),
        }
    }

    pub fn limit(&self, disk: Option<Disk>, held: u64) -> Option<u64> {
        let chosen = self.chosen(disk)?;
        Some(match disk {
            Some(d) => chosen.min(
                held.saturating_add(d.free)
                    .saturating_sub(d.size / 100 * KEEP_FREE_PERCENT),
            ),
            None => chosen,
        })
    }

    fn counts(&self, path: &Path) -> bool {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
            return false;
        };
        let name = name.to_string_lossy();
        Kind::ALL.iter().any(|k| dir == k.dir() && !k.keeps(&name))
            || (self.proxies_dir() == Some(dir) && name.ends_with(".mkv"))
    }

    fn proxies_dir(&self) -> Option<&Path> {
        let dir = &self.proxies;
        (dir.starts_with(crate::dirs::cache()) && !Kind::ALL.iter().any(|k| *dir == k.dir()))
            .then_some(dir.as_path())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub files: usize,
    pub bytes: u64,
    pub chosen: Option<u64>,
    pub limit: Option<u64>,
    pub disk: Option<Disk>,
}

struct Sources {
    policy: fn() -> Policy,
    disk: fn(&Path) -> Option<Disk>,
}

static SOURCES: OnceLock<Sources> = OnceLock::new();

pub fn governed_by(policy: fn() -> Policy, disk: fn(&Path) -> Option<Disk>) {
    let _ = SOURCES.set(Sources { policy, disk });
}

struct Entry {
    used: SystemTime,
    len: u64,
}

struct Index {
    files: HashMap<PathBuf, Entry>,
    held: u64,
    scanned: Instant,
}

impl Index {
    fn of(files: HashMap<PathBuf, Entry>) -> Self {
        Self {
            held: files.values().map(|e| e.len).sum(),
            files,
            scanned: Instant::now(),
        }
    }

    fn put(&mut self, path: &Path, len: u64, used: SystemTime) {
        self.forget(path);
        self.held += len;
        self.files.insert(path.to_path_buf(), Entry { used, len });
    }

    fn forget(&mut self, path: &Path) {
        if let Some(e) = self.files.remove(path) {
            self.held -= e.len;
        }
    }

    fn len_of(&self, path: &Path) -> u64 {
        self.files.get(path).map_or(0, |e| e.len)
    }

    fn rescan(&mut self, policy: &Policy) {
        let mut files = scan(policy);
        for (path, e) in &mut files {
            if let Some(known) = self.files.get(path) {
                e.used = e.used.max(known.used);
            }
        }
        *self = Index::of(files);
    }

    fn shrink(&mut self, limit: u64, started: SystemTime) -> usize {
        if self.held <= limit {
            return 0;
        }
        let mut oldest: Vec<(SystemTime, PathBuf)> = self
            .files
            .iter()
            .map(|(p, e)| (e.used, p.clone()))
            .collect();
        oldest.sort();
        let mut removed = 0;
        for (used, path) in oldest {
            if self.held <= limit {
                break;
            }
            let Ok(meta) = std::fs::metadata(&path) else {
                self.forget(&path);
                continue;
            };
            let modified = meta.modified().unwrap_or(started);
            if modified >= started || modified > used {
                self.put(&path, meta.len(), modified.max(used));
                continue;
            }
            if std::fs::remove_file(&path).is_ok() {
                self.forget(&path);
                removed += 1;
            }
        }
        removed
    }
}

struct State {
    policy: Option<Policy>,
    index: Option<Index>,
    wanted: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    policy: None,
    index: None,
    wanted: false,
});
static WAKE: Condvar = Condvar::new();

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Held<'a> {
    policy: &'a Policy,
    index: &'a mut Index,
    disk: Option<Disk>,
}

fn held<'a>(s: &'a mut State, sources: &Sources) -> Held<'a> {
    let policy = s.policy.get_or_insert_with(sources.policy);
    let index = s.index.get_or_insert_with(|| Index::of(scan(policy)));
    Held {
        policy,
        index,
        disk: (sources.disk)(&crate::dirs::cache()),
    }
}

pub fn read(path: &Path) -> Option<Vec<u8>> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut f, &mut bytes).ok()?;
    mark(path, f.metadata().and_then(|m| m.modified()).ok());
    Some(bytes)
}

pub fn used(path: &Path) {
    mark(
        path,
        std::fs::metadata(path).and_then(|m| m.modified()).ok(),
    );
}

fn mark(path: &Path, modified: Option<SystemTime>) {
    if !path.starts_with(crate::dirs::cache()) {
        return;
    }
    let now = SystemTime::now();
    if let Some(e) = state().index.as_mut().and_then(|i| i.files.get_mut(path)) {
        e.used = now;
    }
    let stale = modified.and_then(|t| now.duration_since(t).ok());
    if stale.is_some_and(|age| age >= MARK_EVERY)
        && let Ok(f) = std::fs::File::options().write(true).open(path)
    {
        let _ = f.set_modified(now);
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
        .and_then(
            |()| match take_room(path, bytes.len() as u64, Some(&part)) {
                true => std::fs::rename(&part, path),
                false => Err(std::io::Error::new(
                    std::io::ErrorKind::StorageFull,
                    "the disk cache has no room for it",
                )),
            },
        );
    if written.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    written
}

pub fn added(path: &Path) -> bool {
    let len = std::fs::metadata(path).map_or(0, |m| m.len());
    let kept = take_room(path, len, None);
    if !kept {
        let _ = std::fs::remove_file(path);
    }
    kept
}

pub fn room(path: &Path) -> Option<u64> {
    let sources = SOURCES.get()?;
    let mut s = state();
    let h = held(&mut s, sources);
    if !h.policy.counts(path) {
        return None;
    }
    h.policy.limit(h.disk, h.index.held - h.index.len_of(path))
}

fn take_room(path: &Path, len: u64, part: Option<&Path>) -> bool {
    let Some(sources) = SOURCES.get() else {
        return true;
    };
    let mut s = state();
    let h = held(&mut s, sources);
    if let Some(part) = part {
        h.index.forget(part);
    }
    if !h.policy.counts(path) {
        return true;
    }
    let others = h.index.held - h.index.len_of(path);
    let limit = h.policy.limit(h.disk, others + len);
    if limit.is_some_and(|l| len > l) {
        return false;
    }
    h.index.put(path, len, SystemTime::now());
    if limit.is_some_and(|l| h.index.held > l) {
        s.wanted = true;
        WAKE.notify_one();
    }
    true
}

pub fn run() {
    settle();
    loop {
        let mut s = state();
        while !s.wanted {
            s = WAKE.wait(s).unwrap_or_else(PoisonError::into_inner);
        }
        s.wanted = false;
        drop(s);
        settle();
    }
}

pub fn settle() {
    let Some(sources) = SOURCES.get() else {
        return;
    };
    shrink(&mut state(), sources);
}

pub fn finish() {
    let Some(sources) = SOURCES.get() else {
        return;
    };
    let mut s = state();
    if s.index.is_some() {
        shrink(&mut s, sources);
    }
}

pub fn reread() {
    let Some(sources) = SOURCES.get() else {
        return;
    };
    let mut s = state();
    let policy = (sources.policy)();
    if s.policy
        .as_ref()
        .is_some_and(|p| p.proxies != policy.proxies)
    {
        s.index = None;
    }
    s.policy = Some(policy);
    s.wanted = true;
    WAKE.notify_one();
}

fn shrink(s: &mut State, sources: &Sources) {
    let h = held(s, sources);
    let over = |h: &Held| {
        h.policy
            .limit(h.disk, h.index.held)
            .filter(|&l| h.index.held > l)
    };
    let Some(mut limit) = over(&h) else {
        return;
    };
    if h.index.scanned.elapsed() >= RESCAN_EVERY {
        h.index.rescan(h.policy);
        match over(&h) {
            Some(l) => limit = l,
            None => return,
        }
    }
    h.index.shrink(limit, SystemTime::now());
}

pub fn usage(policy: &Policy) -> Usage {
    let Some(sources) = SOURCES.get() else {
        let files = scan(policy);
        return Usage {
            files: files.len(),
            bytes: files.values().map(|e| e.len).sum(),
            chosen: None,
            limit: None,
            disk: None,
        };
    };
    let mut s = state();
    let h = held(&mut s, sources);
    Usage {
        files: h.index.files.len(),
        bytes: h.index.held,
        chosen: policy.chosen(h.disk),
        limit: policy.limit(h.disk, h.index.held),
        disk: h.disk,
    }
}

fn scan(policy: &Policy) -> HashMap<PathBuf, Entry> {
    let mut found = HashMap::new();
    for kind in Kind::ALL {
        listed(&kind.dir(), &|name| !kind.keeps(name), &mut found);
    }
    if let Some(dir) = policy.proxies_dir() {
        listed(dir, &|name| name.ends_with(".mkv"), &mut found);
    }
    found
}

fn listed(dir: &Path, counts: &dyn Fn(&str) -> bool, found: &mut HashMap<PathBuf, Entry>) {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return;
    };
    for e in listing.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if !counts(&name) {
            continue;
        }
        let Ok(meta) = e.metadata() else {
            continue;
        };
        if let (true, Ok(used)) = (meta.is_file(), meta.modified()) {
            found.insert(
                e.path(),
                Entry {
                    used,
                    len: meta.len(),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        std::fs::write(dir.join("d.thumbs"), [0u8; 5000]).unwrap();
        let started = now - Duration::from_secs(5);
        let evict = |limit| {
            let mut found = HashMap::new();
            listed(dir, &|_| true, &mut found);
            Index::of(found).shrink(limit, started)
        };
        assert_eq!(evict(8000), 0, "within the limit");
        assert_eq!(evict(7500), 1);
        assert!(!dir.join("a.thumbs").exists(), "the oldest went");
        assert!(dir.join("b.thumbs").exists() && dir.join("c.thumbs").exists());
        assert_eq!(evict(0), 2);
        assert!(
            dir.join("d.thumbs").exists(),
            "a file written since the pass began stays"
        );
    }
}
