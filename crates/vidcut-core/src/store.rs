//! Project files on disk: atomic writes, a per-project lock, and the undo
//! history sidecar that lets `vidcut history undo` work across CLI runs (deviation X4).
//!
//! The project is one pretty-printed JSON file. History lives outside it, in
//! `$XDG_STATE_HOME/vidcut/history/<hash of the project path>.json`, together
//! with the project state it was recorded against. If the file on disk differs
//! from that state, someone edited it externally; the difference becomes one
//! undoable "external change" step.
//!
//! Media and proxy files in or under the project's folder are written
//! relative to it, so the folder moves with its media (to another computer,
//! say); the others stay absolute. In memory every path is absolute.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::editor::{Editor, Entry, History};
use crate::error::{Error, Result};
use crate::model::Project;

/// How long to wait for another process to release the project lock.
const LOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// `$XDG_STATE_HOME/vidcut`, or `~/.local/state/vidcut` and the like
/// ([`crate::dirs::state`]).
pub fn state_dir() -> PathBuf {
    crate::dirs::state()
}

/// FNV-1a: a stable 64-bit hash for sidecar names (not security relevant).
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[derive(Serialize, Deserialize)]
struct Sidecar {
    project_path: PathBuf,
    /// The project as of the last recorded change.
    head: Project,
    undo: Vec<Entry>,
    redo: Vec<Entry>,
}

/// What [`Store::save`] wrote.
#[derive(Debug, Clone)]
pub struct Saved {
    /// The project file's new text.
    pub text: String,
    /// Why the undo history couldn't be written, if so. The edit is saved
    /// all the same, and the history on disk stays as it was: the next load
    /// sees the edit as an outside change, one undo step.
    pub history_error: Option<String>,
}

/// A project file and its sidecar.
#[derive(Debug, Clone)]
pub struct Store {
    path: PathBuf,
    lock_path: PathBuf,
    history_path: PathBuf,
}

impl Store {
    /// Creates a new project file; fails if it exists.
    pub fn create(path: &Path, project: &Project) -> Result<Self> {
        if path.exists() {
            return Err(Error::invalid(format!("{} already exists", path.display())));
        }
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent).map_err(|e| io(path, e))?;
        let dir = crate::dirs::canonical(parent).map_err(|e| io(path, e))?;
        write_atomic(path, &to_json(project, &dir)?).map_err(|e| io(path, e))?;
        let store = Self::open(path)?;
        // A fresh project starts with fresh history.
        let _ = std::fs::remove_file(&store.history_path);
        Ok(store)
    }

    /// Opens an existing project file.
    pub fn open(path: &Path) -> Result<Self> {
        let path = crate::dirs::canonical(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::not_found(format!("no project file {}", path.display()))
            } else {
                io(path, e)
            }
        })?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let lock_path = path.with_file_name(format!(".{name}.lock"));
        let hash = fnv1a(path.as_os_str().as_encoded_bytes());
        let history_path = state_dir()
            .join("history")
            .join(format!("{hash:016x}.json"));
        Ok(Self {
            path,
            lock_path,
            history_path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The project's folder, which relative paths in the file start from.
    fn dir(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new("/"))
    }

    /// The project file's current text.
    pub fn read(&self) -> Result<String> {
        std::fs::read_to_string(&self.path).map_err(|e| io(&self.path, e))
    }

    /// Reads the project and its history without locking (writes are atomic,
    /// so readers always see a whole file).
    pub fn load(&self) -> Result<Editor> {
        self.load_text(&self.read()?)
    }

    /// Like [`load`](Self::load), for file text the caller already read.
    pub fn load_text(&self, text: &str) -> Result<Editor> {
        let project = parse(&self.path, self.dir(), text)?;
        let sidecar = std::fs::read_to_string(&self.history_path)
            .ok()
            .and_then(|t| serde_json::from_str::<Sidecar>(&t).ok())
            .filter(|s| s.project_path == self.path);
        Ok(match sidecar {
            Some(s) if s.head == project => Editor::new(
                project,
                History {
                    undo: s.undo,
                    redo: s.redo,
                },
            ),
            Some(s) => {
                let mut editor = Editor::new(
                    s.head,
                    History {
                        undo: s.undo,
                        redo: s.redo,
                    },
                );
                editor.record_external(project);
                editor
            }
            None => Editor::new(project, History::default()),
        })
    }

    /// Runs `f` on the project under an exclusive lock and saves the result if
    /// the project changed. Also gives [`Saved::history_error`], if any.
    pub fn transact<T>(
        &self,
        f: impl FnOnce(&mut Editor) -> Result<T>,
    ) -> Result<(T, Option<String>)> {
        let _lock = self.lock()?;
        let mut editor = self.load()?;
        let before = editor.project().clone();
        let out = f(&mut editor)?;
        let history_error = if editor.project() != &before {
            self.save(&editor)?.history_error
        } else {
            None
        };
        Ok((out, history_error))
    }

    /// Writes the project and its history. Callers hold the
    /// [`lock`](Self::lock).
    pub fn save(&self, editor: &Editor) -> Result<Saved> {
        let text = to_json(editor.project(), self.dir())?;
        let sidecar = Sidecar {
            project_path: self.path.clone(),
            head: editor.project().clone(),
            undo: editor.history().undo.clone(),
            redo: editor.history().redo.clone(),
        };
        // The sidecar goes first: whoever sees the new project file (a file
        // watcher, say) then also finds the history that matches it, instead
        // of mistaking the change for an external edit.
        // Failing to save the history must not lose the edit. A failed write
        // leaves the old sidecar whole (`write_atomic`).
        let dir = self.history_path.parent().expect("history dir");
        let json = serde_json::to_string(&sidecar).map_err(|e| Error::internal(e.to_string()))?;
        let history_error = std::fs::create_dir_all(dir)
            .and_then(|()| write_atomic(&self.history_path, &json))
            .err()
            .map(|e| format!("{}: {e}", self.history_path.display()));
        write_atomic(&self.path, &text).map_err(|e| io(&self.path, e))?;
        Ok(Saved {
            text,
            history_error,
        })
    }

    /// Takes the project's write lock (released when the returned file is
    /// dropped), waiting up to 30 s for other writers.
    pub fn lock(&self) -> Result<File> {
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&self.lock_path)
            .map_err(|e| io(&self.lock_path, e))?;
        let started = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(std::fs::TryLockError::WouldBlock) if started.elapsed() < LOCK_TIMEOUT => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    return Err(Error::conflict(format!(
                        "{} is locked by another process",
                        self.path.display()
                    )));
                }
                Err(std::fs::TryLockError::Error(e)) => return Err(io(&self.lock_path, e)),
            }
        }
    }
}

fn io(path: &Path, e: std::io::Error) -> Error {
    Error::internal(format!("{}: {e}", path.display()))
}

/// The paths of the files a project names: its media and their proxies.
fn files(project: &mut Project) -> impl Iterator<Item = &mut PathBuf> {
    project
        .media
        .iter_mut()
        .flat_map(|m| std::iter::once(&mut m.path).chain(m.proxy.as_mut().map(|p| &mut p.path)))
}

/// The file's text, with the paths in or under `dir` relative to it.
fn to_json(project: &Project, dir: &Path) -> Result<String> {
    let mut project = project.clone();
    for path in files(&mut project) {
        if let Ok(relative) = path.strip_prefix(dir) {
            *path = portable(relative);
        }
    }
    let mut s =
        serde_json::to_string_pretty(&project).map_err(|e| Error::internal(e.to_string()))?;
    s.push('\n');
    Ok(s)
}

/// A relative path as the file keeps it: its parts joined by `/` on every
/// system, so a project folder moves between Linux and Windows.
fn portable(relative: &Path) -> PathBuf {
    #[cfg(windows)]
    let path = PathBuf::from(relative.to_string_lossy().replace('\\', "/"));
    #[cfg(not(windows))]
    let path = relative.to_path_buf();
    path
}

/// A relative path the file keeps, as this system writes it.
fn native(relative: &Path) -> PathBuf {
    #[cfg(windows)]
    let path = PathBuf::from(relative.to_string_lossy().replace('/', "\\"));
    #[cfg(not(windows))]
    let path = relative.to_path_buf();
    path
}

/// Whether a path the file keeps is absolute on some system: `/…`, or a
/// Windows drive's (`C:\…`, `C:/…`) or server's (`\\server\…`). One written
/// on the other kind of system names nothing here, so it is kept as it was
/// written rather than joined onto the project's folder: it shows as it
/// was, and goes back into the file unchanged.
fn absolute_anywhere(path: &Path) -> bool {
    let Some(s) = path.to_str() else {
        return path.is_absolute();
    };
    let b = s.as_bytes();
    s.starts_with(['/', '\\'])
        || (b.len() > 2
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && matches!(b[2], b'\\' | b'/'))
}

/// The project in the file's `text`, its relative paths made absolute from
/// `dir`.
fn parse(path: &Path, dir: &Path, text: &str) -> Result<Project> {
    let mut project: Project = serde_json::from_str(text)
        .map_err(|e| Error::invalid(format!("{} is not a vidcut project: {e}", path.display())))?;
    for file in files(&mut project) {
        if !absolute_anywhere(file) {
            *file = dir.join(native(file));
        }
    }
    let problems = crate::validate::violations(&project);
    if !problems.is_empty() {
        return Err(Error::invalid(format!(
            "{} is inconsistent: {}",
            path.display(),
            problems.join("; ")
        )));
    }
    Ok(project)
}

/// Writes `data` to a temporary file next to `path`, syncs it and renames it
/// over `path`, so readers see either the old or the new file.
fn write_atomic(path: &Path, data: &str) -> std::io::Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut f = File::create(&tmp)?;
        f.write_all(data.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}
