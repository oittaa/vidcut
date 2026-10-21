use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, SystemTime};

use crate::fingerprint::Origin;
use crate::lru::Lru;
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
        let path = plain(path);
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
            return false;
        };
        let name = name.to_string_lossy();
        Kind::ALL
            .iter()
            .any(|k| dir == plain(&k.dir()) && !k.keeps(&name))
            || (self.proxies_dir().as_deref() == Some(dir) && name.ends_with(".mkv"))
    }

    fn proxies_dir(&self) -> Option<PathBuf> {
        let dir = plain(&self.proxies);
        (dir.starts_with(plain(&crate::dirs::cache()))
            && !Kind::ALL.iter().any(|k| dir == plain(&k.dir())))
        .then_some(dir)
    }
}

fn plain(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir if out.pop() => {}
            c => out.push(c),
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub files: usize,
    pub bytes: u64,
    pub chosen: Option<u64>,
    pub limit: Option<u64>,
    pub disk: Option<Disk>,
}

pub const REBUILD_EVERY: Duration = Duration::from_secs(60 * 60);

const UNFINISHED_FOR: Duration = Duration::from_secs(24 * 60 * 60);

fn unfinished(name: &str) -> bool {
    name.split('.').any(|piece| piece == "part")
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    len: u64,
    touched: bool,
    written: u64,
}

struct Index {
    files: Lru<PathBuf, Entry, { usize::MAX }>,
    total: u64,
    writes: u64,
}

impl Index {
    fn of(found: Vec<(PathBuf, u64, SystemTime)>) -> Self {
        let mut index = Index {
            files: Lru::new(),
            total: 0,
            writes: 0,
        };
        for (path, len, _) in found {
            index.put(path, len, false, 0);
        }
        index
    }

    fn put(&mut self, path: PathBuf, len: u64, touched: bool, written: u64) {
        self.forget(&path);
        self.total += len;
        self.files.insert(
            path,
            Entry {
                len,
                touched,
                written,
            },
        );
    }

    fn forget(&mut self, path: &Path) {
        if let Some((_, e)) = self.files.remove(&path.to_path_buf()) {
            self.total -= e.len;
        }
    }

    fn fit(&mut self, limit: u64, victims: &mut Vec<PathBuf>) {
        while self.total > limit {
            let Some((path, e)) = self.files.pop_oldest() else {
                break;
            };
            self.total -= e.len;
            victims.push(path);
        }
    }
}

struct Live {
    index: Index,
    policy: Policy,
}

impl Live {
    fn limit(&self, disk: Option<Disk>) -> Option<u64> {
        self.policy.limit(disk, self.index.total)
    }
}

static LIVE: Mutex<Option<Live>> = Mutex::new(None);

static DISK: OnceLock<fn(&Path) -> Option<Disk>> = OnceLock::new();

fn free_space() -> Option<Disk> {
    DISK.get().and_then(|disk| disk(&crate::dirs::cache()))
}

fn live() -> MutexGuard<'static, Option<Live>> {
    LIVE.lock().unwrap_or_else(PoisonError::into_inner)
}

fn drop_all(victims: Vec<PathBuf>) {
    for path in victims {
        let _ = std::fs::remove_file(path);
    }
}

pub fn start(policy: Policy, disk: fn(&Path) -> Option<Disk>) {
    let _ = DISK.set(disk);
    let (found, left) = found(&policy);
    let index = Index::of(found);
    let free = free_space();
    let mut victims = Vec::new();
    {
        let mut g = live();
        let live = g.insert(Live { index, policy });
        if let Some(limit) = live.limit(free) {
            live.index.fit(limit, &mut victims);
        }
    }
    drop_all(victims);
    drop_all(left);
}

pub fn set_policy(policy: Policy) {
    let free = free_space();
    let mut victims = Vec::new();
    {
        let mut g = live();
        let Some(live) = g.as_mut() else {
            return;
        };
        live.policy = policy;
        if let Some(limit) = live.limit(free) {
            live.index.fit(limit, &mut victims);
        }
    }
    drop_all(victims);
}

pub fn rebuild() {
    let Some((policy, mark)) = live().as_ref().map(|l| (l.policy.clone(), l.index.writes)) else {
        return;
    };
    let (found, left) = found(&policy);
    let free = free_space();
    let mut victims = Vec::new();
    {
        let mut g = live();
        let Some(live) = g.as_mut() else {
            return;
        };
        let old = &live.index;
        let mut fresh = Index {
            files: Lru::new(),
            total: 0,
            writes: old.writes,
        };
        for (path, len, _) in found {
            let touched = old.files.peek(&path).is_some_and(|e| e.touched);
            fresh.put(path, len, touched, 0);
        }
        for (path, e) in old.files.iter().filter(|(_, e)| e.written > mark) {
            fresh.put(path.clone(), e.len, e.touched, e.written);
        }
        live.index = fresh;
        if let Some(limit) = live.limit(free) {
            live.index.fit(limit, &mut victims);
        }
    }
    drop_all(victims);
    drop_all(left);
}

enum Reserved {
    Free,
    Room(Vec<PathBuf>),
    Full,
}

fn reserve(path: &Path, len: u64) -> Reserved {
    let path = plain(path);
    let free = free_space();
    let mut g = live();
    let Some(live) = g.as_mut() else {
        return Reserved::Free;
    };
    if !live.policy.counts(&path) {
        return Reserved::Free;
    }
    let mut victims = Vec::new();
    if let Some(limit) = live.limit(free) {
        if len > limit {
            return Reserved::Full;
        }
        live.index.forget(&path);
        live.index.fit(limit.saturating_sub(len), &mut victims);
    }
    live.index.writes += 1;
    let written = live.index.writes;
    live.index.put(path, len, true, written);
    Reserved::Room(victims)
}

fn forget(path: &Path) {
    if let Some(live) = live().as_mut() {
        live.index.forget(&plain(path));
    }
}

pub fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    match reserve(path, bytes.len() as u64) {
        Reserved::Full => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "the disk cache has no room for it",
            ));
        }
        Reserved::Room(victims) => drop_all(victims),
        Reserved::Free => {}
    }
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| crate::temp::write(path, bytes));
    if written.is_err() {
        forget(path);
    }
    written
}

pub fn added(path: &Path) -> bool {
    let len = std::fs::metadata(path).map_or(0, |m| m.len());
    match reserve(path, len) {
        Reserved::Full => {
            let _ = std::fs::remove_file(path);
            forget(path);
            false
        }
        Reserved::Room(victims) => {
            drop_all(victims);
            true
        }
        Reserved::Free => true,
    }
}

pub fn room(path: &Path) -> Option<u64> {
    let path = plain(path);
    let free = free_space();
    let g = live();
    let live = g.as_ref()?;
    if !live.policy.counts(&path) {
        return None;
    }
    live.limit(free)
}

fn note(path: &Path, len: u64) -> (bool, Vec<PathBuf>) {
    let path = plain(path);
    let mut victims = Vec::new();
    let mut g = live();
    let Some(live) = g.as_mut() else {
        return (false, victims);
    };
    if !live.policy.counts(&path) {
        return (false, victims);
    }
    if let Some(e) = live.index.files.get_mut(&path) {
        let first = !e.touched;
        e.touched = true;
        return (first, victims);
    }
    live.index.writes += 1;
    let written = live.index.writes;
    live.index.put(path, len, true, written);
    if let Some(limit) = live.limit(free_space()) {
        live.index.fit(limit, &mut victims);
    }
    (true, victims)
}

fn touch(path: &Path) {
    if let Ok(f) = std::fs::File::options().write(true).open(path) {
        let _ = f.set_modified(SystemTime::now());
    }
}

pub fn read(path: &Path) -> Option<Vec<u8>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => {
            forget(path);
            return None;
        }
    };
    let (first, victims) = note(path, bytes.len() as u64);
    drop_all(victims);
    if first {
        touch(path);
    }
    Some(bytes)
}

pub fn used(path: &Path) {
    let Ok(meta) = std::fs::metadata(path) else {
        forget(path);
        return;
    };
    let (first, victims) = note(path, meta.len());
    drop_all(victims);
    if first {
        touch(path);
    }
}

pub fn usage(policy: &Policy, disk: Option<Disk>) -> Usage {
    let (files, bytes) = match live().as_ref() {
        Some(l) => (l.index.files.len(), l.index.total),
        None => {
            let (found, _) = found(policy);
            (found.len(), found.iter().map(|(_, len, _)| len).sum())
        }
    };
    Usage {
        files,
        bytes,
        chosen: policy.chosen(disk),
        limit: policy.limit(disk, bytes),
        disk,
    }
}

type Found = Vec<(PathBuf, u64, SystemTime)>;

fn found(policy: &Policy) -> (Found, Vec<PathBuf>) {
    let (mut found, mut left) = (Vec::new(), Vec::new());
    for kind in Kind::ALL {
        let counts = |name: &str| !kind.keeps(name);
        listed(&plain(&kind.dir()), &counts, &mut found, &mut left);
    }
    if let Some(dir) = policy.proxies_dir() {
        listed(&dir, &|name| name.ends_with(".mkv"), &mut found, &mut left);
    }
    found.sort_by_key(|(_, _, used)| *used);
    (found, left)
}

fn listed(dir: &Path, counts: &dyn Fn(&str) -> bool, found: &mut Found, left: &mut Vec<PathBuf>) {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return;
    };
    for e in listing.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        let part = unfinished(&name);
        if !part && !counts(&name) {
            continue;
        }
        let Ok(meta) = e.metadata() else {
            continue;
        };
        let (true, Ok(used)) = (meta.is_file(), meta.modified()) else {
            continue;
        };
        if !part {
            found.push((e.path(), meta.len(), used));
        } else if used.elapsed().is_ok_and(|age| age > UNFINISHED_FOR) {
            left.push(e.path());
        }
    }
}
