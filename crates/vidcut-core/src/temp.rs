use std::fs::{File, OpenOptions};
use std::hash::{BuildHasher, Hasher};
use std::io::{ErrorKind, Result, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Temp {
    path: PathBuf,
    dir: bool,
    kept: bool,
}

const TRIES: usize = 100;

static LEFT: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

fn left() -> MutexGuard<'static, Vec<PathBuf>> {
    LEFT.lock().unwrap_or_else(PoisonError::into_inner)
}

fn random() -> u64 {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(COUNT.fetch_add(1, Ordering::Relaxed));
    h.write_u32(std::process::id());
    h.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    h.finish()
}

fn name(target: &Path) -> String {
    let base: String = target.file_name().map_or_else(
        || "file".into(),
        |n| n.to_string_lossy().chars().take(48).collect(),
    );
    let ext = target
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    format!(".{base}.{:016x}.part{ext}", random())
}

fn made<T>(target: &Path, dir: bool, make: impl Fn(&Path) -> Result<T>) -> Result<(Temp, T)> {
    let folder = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    for _ in 0..TRIES {
        let path = folder.join(name(target));
        match make(&path) {
            Ok(made) => {
                left().push(path.clone());
                let temp = Temp {
                    path,
                    dir,
                    kept: false,
                };
                return Ok((temp, made));
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        ErrorKind::AlreadyExists,
        format!("no free name beside {}", target.display()),
    ))
}

pub fn file(target: &Path) -> Result<(Temp, File)> {
    made(target, false, |p| {
        OpenOptions::new().write(true).create_new(true).open(p)
    })
}

pub fn dir(target: &Path) -> Result<Temp> {
    made(target, true, |p| std::fs::create_dir(p)).map(|(temp, ())| temp)
}

pub fn write(target: &Path, bytes: &[u8]) -> Result<()> {
    let (temp, mut f) = file(target)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    temp.keep(target)
}

impl Temp {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn keep(mut self, target: &Path) -> Result<()> {
        std::fs::rename(&self.path, target)?;
        self.kept = true;
        Ok(())
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        left().retain(|p| *p != self.path);
        if self.kept {
            return;
        }
        let _ = if self.dir {
            std::fs::remove_dir_all(&self.path)
        } else {
            std::fs::remove_file(&self.path)
        };
    }
}

pub fn remove_left() {
    for path in std::mem::take(&mut *left()) {
        let _ = std::fs::remove_file(&path).or_else(|_| std::fs::remove_dir_all(&path));
    }
}

pub fn lock(path: &Path) -> Result<File> {
    let mut name = path.as_os_str().to_owned();
    name.push(".lock");
    let f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(PathBuf::from(name))?;
    match f.lock() {
        Ok(()) => Ok(f),
        Err(e) if e.kind() == ErrorKind::Unsupported => Ok(f),
        Err(e) => Err(e),
    }
}
