//! Project files on disk: atomic writes, a per-project lock, and the undo
//! history sidecar that lets `vidcut undo` work across CLI runs (deviation X4).
//!
//! The project is one pretty-printed JSON file. History lives outside it, in
//! `$XDG_STATE_HOME/vidcut/history/<hash of the project path>.json`, together
//! with the project state it was recorded against. If the file on disk differs
//! from that state and the sidecar still parses, the difference becomes one
//! undoable "external change" step. A missing or unreadable sidecar loads
//! with an empty history: the edit stays, and there is no undo step.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::editor::{Editor, Entry, History};
use crate::error::{Error, Result};
use crate::model::{FORMAT_VERSION, Project};

/// How long to wait for another process to release the project lock.
const LOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// `$XDG_STATE_HOME`, or `~/.local/state`.
pub fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local").join("state"))
        .join("vidcut")
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
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

/// What the next [`Store::load`] can undo when the history file was not updated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryLoad {
    /// The previous sidecar still parses, and it is not this project. The
    /// next load records one outside change.
    OutsideChange,
    /// The previous sidecar still parses and already matches this project.
    /// The next load keeps that history.
    Matches,
    /// The sidecar is missing or unreadable. The next load has an empty
    /// history, so this edit has no undo step.
    Empty,
}

/// A history sidecar that could not be written. The project file was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryError {
    /// `{path}: {io error}`.
    pub message: String,
    /// What the next load does with undo.
    pub on_load: HistoryLoad,
}

/// What [`Store::save`] wrote.
#[derive(Debug, Clone)]
pub struct Saved {
    /// The project file's new text.
    pub text: String,
    /// Set when the history file could not be written. The edit is in the
    /// project file. [`HistoryError::on_load`] says what a later load can undo.
    pub history_error: Option<HistoryError>,
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
        write_atomic(path, &to_json(project)?).map_err(|e| io(path, e))?;
        let store = Self::open(path)?;
        // A fresh project starts with fresh history.
        let _ = std::fs::remove_file(&store.history_path);
        Ok(store)
    }

    /// Opens an existing project file.
    pub fn open(path: &Path) -> Result<Self> {
        let path = path.canonicalize().map_err(|e| {
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
        let project = parse(&self.path, text)?;
        let sidecar = self.read_sidecar();
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
    ) -> Result<(T, Option<HistoryError>)> {
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
        let text = to_json(editor.project())?;
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
        let history_error = match std::fs::create_dir_all(dir)
            .and_then(|()| write_atomic(&self.history_path, &json))
        {
            Ok(()) => None,
            Err(e) => Some(HistoryError {
                message: format!("{}: {e}", self.history_path.display()),
                // The failed write left the previous sidecar whole, so this
                // is what the next load will do with the project just written.
                on_load: match self.read_sidecar() {
                    Some(s) if s.head != *editor.project() => HistoryLoad::OutsideChange,
                    Some(_) => HistoryLoad::Matches,
                    None => HistoryLoad::Empty,
                },
            }),
        };
        write_atomic(&self.path, &text).map_err(|e| io(&self.path, e))?;
        Ok(Saved {
            text,
            history_error,
        })
    }

    /// The sidecar for this project, if it parses. A missing file, bad JSON,
    /// or another project's path is `None`: the same cases [`load_text`](Self::load_text)
    /// treats as an empty history.
    fn read_sidecar(&self) -> Option<Sidecar> {
        let text = std::fs::read_to_string(&self.history_path).ok()?;
        let sidecar: Sidecar = serde_json::from_str(&text).ok()?;
        (sidecar.project_path == self.path).then_some(sidecar)
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

fn to_json(project: &Project) -> Result<String> {
    let mut s =
        serde_json::to_string_pretty(project).map_err(|e| Error::internal(e.to_string()))?;
    s.push('\n');
    Ok(s)
}

fn parse(path: &Path, text: &str) -> Result<Project> {
    let project: Project = serde_json::from_str(text)
        .map_err(|e| Error::invalid(format!("{} is not a vidcut project: {e}", path.display())))?;
    if project.vidcut > FORMAT_VERSION {
        return Err(Error::invalid(format!(
            "{} uses project format {}, newer than this vidcut ({FORMAT_VERSION})",
            path.display(),
            project.vidcut
        )));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Env;
    use crate::model::Settings;
    use crate::request::Envelope;
    use crate::test_support::FakeEnv;
    use serde_json::json;

    fn lock_state() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn isolated_state(dir: &Path) {
        // SAFETY: callers hold `lock_state`, so tests in this module never
        // read XDG_STATE_HOME concurrently with a different value.
        unsafe { std::env::set_var("XDG_STATE_HOME", dir.join("state")) };
    }

    fn apply(store: &Store, env: &dyn Env, req: serde_json::Value) -> Result<crate::Response> {
        store
            .transact(|ed| ed.apply(Envelope::from_json(req).map_err(Error::usage)?, env, None))
            .map(|(r, _)| r)
    }

    #[test]
    fn edits_persist_with_undo_across_opens() {
        let _state = lock_state();
        let dir = std::env::temp_dir().join(format!("vidcut-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        isolated_state(&dir);
        let env = FakeEnv::default().with(
            "/m.mp4",
            crate::test_support::media_info(4.0, crate::time::Rational::new(25, 1).unwrap(), true),
        );
        let path = dir.join("p.vidcut");
        let store = Store::create(&path, &Project::new(Settings::default())).unwrap();
        assert!(Store::create(&path, &Project::new(Settings::default())).is_err());
        apply(
            &store,
            &env,
            json!({"method": "media.add", "params": {"paths": ["/m.mp4"]}}),
        )
        .unwrap();
        apply(
            &store,
            &env,
            json!({"method": "edit.append", "params": {"media": "m1"}}),
        )
        .unwrap();

        // A second "process" sees the clip and can undo it.
        let store2 = Store::open(&path).unwrap();
        assert_eq!(
            store2
                .load()
                .unwrap()
                .project()
                .track("V1")
                .unwrap()
                .clips
                .len(),
            1
        );
        apply(
            &store2,
            &env,
            json!({"method": "history.undo", "params": {}}),
        )
        .unwrap();
        assert!(
            store
                .load()
                .unwrap()
                .project()
                .track("V1")
                .unwrap()
                .clips
                .is_empty()
        );

        // An external edit becomes one undoable step.
        let mut text = std::fs::read_to_string(&path).unwrap();
        text = text.replace("\"ripple_on\": true", "\"ripple_on\": false");
        std::fs::write(&path, text).unwrap();
        let ed = store.load().unwrap();
        assert!(!ed.project().settings.ripple_on);
        assert_eq!(ed.history().undo.last().unwrap().label, "external change");
        apply(
            &store,
            &env,
            json!({"method": "history.undo", "params": {}}),
        )
        .unwrap();
        assert!(store.load().unwrap().project().settings.ripple_on);

        // Queries and failed edits leave the file untouched.
        let before = std::fs::read_to_string(&path).unwrap();
        apply(
            &store,
            &env,
            json!({"method": "timeline.get", "params": {}}),
        )
        .unwrap();
        assert!(
            apply(
                &store,
                &env,
                json!({"method": "edit.delete", "params": {"clips": "c9"}})
            )
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

        // A history that can't be written (a folder sits where it goes)
        // doesn't lose the edit, and says why.
        let other =
            Store::create(&dir.join("q.vidcut"), &Project::new(Settings::default())).unwrap();
        std::fs::create_dir_all(&other.history_path).unwrap();
        let (_, history_error) = other
            .transact(|ed| {
                let e = Envelope::from_json(
                    json!({"method": "media.add", "params": {"paths": ["/m.mp4"]}}),
                );
                ed.apply(e.unwrap(), &env, None)
            })
            .unwrap();
        let history_error = history_error.expect("sidecar write fails");
        assert!(history_error.message.contains(".json"), "{history_error:?}");
        // A directory where the sidecar goes does not parse, so the next
        // load must not invent an undo step for the edit it kept.
        assert_eq!(history_error.on_load, HistoryLoad::Empty);
        let loaded = other.load().unwrap();
        assert_eq!(loaded.project().media.len(), 1);
        assert!(loaded.history().undo.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_sidecar_write_keeps_the_edit_and_a_missing_sidecar_has_no_undo() {
        let _state = lock_state();
        let dir = std::env::temp_dir().join(format!("vidcut-store-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        isolated_state(&dir);
        let env = FakeEnv::default().with(
            "/m.mp4",
            crate::test_support::media_info(4.0, crate::time::Rational::new(25, 1).unwrap(), true),
        );
        let path = dir.join("p.vidcut");
        let store = Store::create(&path, &Project::new(Settings::default())).unwrap();
        apply(
            &store,
            &env,
            json!({"method": "media.add", "params": {"paths": ["/m.mp4"]}}),
        )
        .unwrap();
        assert!(store.history_path.is_file());

        // Sit on the temp name `write_atomic` uses, so the replace fails and
        // the previous sidecar stays a file that still parses.
        let name = store.history_path.file_name().unwrap().to_string_lossy();
        let block = store
            .history_path
            .with_file_name(format!(".{name}.{}.tmp", std::process::id()));
        std::fs::create_dir(&block).unwrap();
        // The sidecar already matches this project, so a failed rewrite does
        // not become an outside change.
        let same = store.load().unwrap();
        let saved = {
            let _lock = store.lock().unwrap();
            store.save(&same).unwrap()
        };
        assert_eq!(
            saved.history_error.expect("rewrite fails").on_load,
            HistoryLoad::Matches
        );

        let (_, history_error) = store
            .transact(|ed| {
                let e = Envelope::from_json(json!({
                    "method": "project.set",
                    "params": {"ripple_on": false}
                }))
                .map_err(Error::usage)?;
                ed.apply(e, &env, None)
            })
            .unwrap();
        let history_error = history_error.expect("sidecar write fails");
        assert_eq!(history_error.on_load, HistoryLoad::OutsideChange);
        assert!(
            history_error.message.contains(name.as_ref()),
            "{history_error:?}"
        );

        // The failed write kept the edit. The previous sidecar still parses,
        // so the next load records one outside change.
        let loaded = store.load().unwrap();
        assert!(!loaded.project().settings.ripple_on);
        assert_eq!(loaded.project().media.len(), 1);
        assert_eq!(
            loaded.history().undo.last().unwrap().label,
            "external change"
        );

        // Delete that sidecar. The edit stays, and nothing invents an undo step.
        std::fs::remove_dir(&block).unwrap();
        std::fs::remove_file(&store.history_path).unwrap();
        let loaded = store.load().unwrap();
        assert!(!loaded.project().settings.ripple_on);
        assert_eq!(loaded.project().media.len(), 1);
        assert!(
            loaded.history().undo.is_empty(),
            "a missing sidecar is not an undo step"
        );
        assert!(loaded.history().redo.is_empty());

        // Unreadable JSON is the same: empty history, edit kept.
        std::fs::write(&store.history_path, "{not json").unwrap();
        let loaded = store.load().unwrap();
        assert!(loaded.history().undo.is_empty());
        assert!(!loaded.project().settings.ripple_on);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
