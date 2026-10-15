//! Project files on disk: atomic writes, a per-project lock, and the undo
//! history that lets `vidcut history undo` work across CLI runs (deviation
//! X4, `crate::history`).
//!
//! The project is one pretty-printed JSON file. Its history is a file of
//! its own, `$XDG_STATE_HOME/vidcut/history/<hash of the project
//! path>.jsonl`. If the project file's hash isn't the one the history's
//! last save wrote, someone edited it outside: the difference becomes one
//! undoable "external change" step.
//!
//! Media and proxy files in or under the project's folder are written
//! relative to it, so the folder moves with its media (to another computer,
//! say); the others stay absolute. In memory every path is absolute.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::editor::{Editor, History};
use crate::error::{Error, Result};
use crate::history::{self, HistoryFile, Tail, text_hash};
use crate::model::Project;

/// How long to wait for another process to release the project lock.
const LOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// `$XDG_STATE_HOME/vidcut`, or `~/.local/state/vidcut` and the like
/// ([`crate::dirs::state`]).
pub fn state_dir() -> PathBuf {
    crate::dirs::state()
}

/// What [`Store::save`] wrote.
#[derive(Debug, Clone)]
pub struct Saved {
    /// The project file's new text.
    pub text: String,
    /// Why the undo history couldn't be written, if so. The edit is saved
    /// all the same, and the history starts again at the next save.
    pub history_error: Option<String>,
    /// The history holds more steps than its limit: [`Store::compact`]
    /// trims it.
    pub compact: bool,
}

/// A project file and its undo history.
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
        Self::replace(path, project)
    }

    /// Creates a new project file over whatever is at `path`.
    pub fn replace(path: &Path, project: &Project) -> Result<Self> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent).map_err(|e| io(path, e))?;
        let dir = crate::dirs::canonical(parent).map_err(|e| io(path, e))?;
        write_atomic(path, &to_json(&mut project.clone(), &dir)?).map_err(|e| io(path, e))?;
        let store = Self::open(path)?;
        // A fresh project starts with fresh history.
        let _ = std::fs::remove_file(&store.history_path);
        Ok(store)
    }

    /// Opens an existing project file.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_in(path, &state_dir())
    }

    /// [`open`](Self::open), its history in `state` (`state_dir`).
    fn open_in(path: &Path, state: &Path) -> Result<Self> {
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
        let hash = crate::hash64(path.as_os_str().as_encoded_bytes());
        let history_path = state.join("history").join(format!("{hash:016x}.jsonl"));
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

    /// The project file's size and time of change: a write changes them.
    pub fn stamp(&self) -> Option<(u64, std::time::SystemTime)> {
        let meta = std::fs::metadata(&self.path).ok()?;
        Some((meta.len(), meta.modified().ok()?))
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
        let file = self.history_file();
        let hash = text_hash(text);
        let Some((tail, end)) = file.tail() else {
            return Ok(Editor::new(project, History::default()));
        };
        if tail.hash == hash {
            return Ok(Editor::on_file(project, file, hash));
        }
        // Edited outside: the project as the history's last save left it,
        // and then the file as it is now, one step.
        let Some(last) = last_saved(&file, end) else {
            return Ok(Editor::new(project, History::default()));
        };
        let mut editor = Editor::on_file(last, file, tail.hash);
        editor.record_external(project);
        Ok(editor)
    }

    /// The project, for requests that only read it: its history is read
    /// only when one asks for it.
    pub fn load_project(&self) -> Result<Editor> {
        let text = self.read()?;
        let project = parse(&self.path, self.dir(), &text)?;
        Ok(Editor::on_file(
            project,
            self.history_file(),
            text_hash(&text),
        ))
    }

    fn history_file(&self) -> HistoryFile {
        HistoryFile::new(self.history_path.clone(), self.path.clone())
    }

    /// Runs `f` on the project under an exclusive lock and saves the result if
    /// the project changed. Also gives [`Saved::history_error`], if any.
    pub fn transact<T>(
        &self,
        f: impl FnOnce(&mut Editor) -> Result<T>,
    ) -> Result<(T, Option<String>)> {
        let lock = self.lock()?;
        let mut editor = self.load()?;
        // Every change raises the revision.
        let rev = editor.project().rev;
        let out = f(&mut editor)?;
        if editor.project().rev == rev {
            return Ok((out, None));
        }
        let saved = self.save(&mut editor)?;
        drop(lock);
        if saved.compact {
            // A trim is a rare and short wait for a command line.
            let _ = self.compact();
        }
        Ok((out, saved.history_error))
    }

    /// Writes the project and its history. Callers hold the
    /// [`lock`](Self::lock).
    pub fn save(&self, editor: &mut Editor) -> Result<Saved> {
        let text = to_json(editor.project_mut(), self.dir())?;
        // The history goes first: whoever sees the new project file (a file
        // watcher, say) then also finds the history that matches it, instead
        // of mistaking the change for an external edit. Failing to save the
        // history must not lose the edit.
        let history = self.save_history(editor, &text_hash(&text));
        if history.is_err() {
            editor.lost_file();
        }
        write_atomic(&self.path, &text).map_err(|e| io(&self.path, e))?;
        Ok(Saved {
            text,
            compact: history.as_ref().is_ok_and(history::due),
            history_error: history
                .err()
                .map(|e| format!("{}: {e}", self.history_path.display())),
        })
    }

    /// Adds what happened to `editor`'s history since it was last saved to
    /// the history file, or writes the file anew. The file's new tail.
    fn save_history(&self, editor: &mut Editor, hash: &str) -> std::io::Result<Tail> {
        let moved = editor.take_moved();
        let file = self.history_file();
        // Where this editor left it, unless another save came between.
        if let Some((_, stands_on)) = &editor.file
            && let Some((tail, end)) = file.tail()
            && tail.hash == *stands_on
        {
            let tail = file.append(&tail, end, moved, hash)?;
            editor.saved_to(file, hash.into());
            return Ok(tail);
        }
        // The steps the editor holds, after the project before them.
        let h = editor.held();
        let mut base = serde_json::to_value(editor.project()).map_err(std::io::Error::other)?;
        for e in h.undo.iter().rev() {
            e.change
                .apply(&mut base, false)
                .map_err(std::io::Error::other)?;
        }
        let tail = file.write_whole(&base, h, hash)?;
        editor.saved_to(file, hash.into());
        Ok(tail)
    }

    /// Trims the history file when it holds more steps than its limit
    /// (`crate::history`). The project lock is taken only to put the new
    /// file in place, so it may run on a thread of its own.
    pub fn compact(&self) -> Result<()> {
        self.history_file()
            .compact(|| {
                self.lock()
                    .map_err(|e| std::io::Error::other(e.to_string()))
            })
            .map_err(|e| io(&self.history_path, e))
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

/// The project as the history's last save left it: its first project with
/// each step to undo made again. `None` when they don't fit.
fn last_saved(file: &HistoryFile, end: u64) -> Option<Project> {
    let (mut json, h) = file.read_all(end).ok()?;
    for e in &h.undo {
        e.change.apply(&mut json, true).ok()?;
    }
    serde_json::from_value(json).ok()
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

/// The file's text, with the paths in or under `dir` relative to it. They
/// are relative while it is written, and as they were after.
fn to_json(project: &mut Project, dir: &Path) -> Result<String> {
    let absolute: Vec<Option<PathBuf>> = files(project)
        .map(|path| {
            let relative = path.strip_prefix(dir).ok().map(portable);
            relative.map(|r| std::mem::replace(path, r))
        })
        .collect();
    let text = serde_json::to_string_pretty(project);
    for (path, was) in files(project).zip(absolute) {
        if let Some(was) = was {
            *path = was;
        }
    }
    let mut s = text.map_err(|e| Error::internal(e.to_string()))?;
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
/// was, and goes back into the file unchanged. Its media are missing
/// ([`from_another_system`]).
fn absolute_anywhere(path: &Path) -> bool {
    let Some(s) = path.to_str() else {
        return path.is_absolute();
    };
    s.starts_with(['/', '\\']) || windows_absolute(s)
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
pub(crate) fn write_atomic(path: &Path, data: &str) -> std::io::Result<()> {
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

/// Whether `path` is absolute only on the other kind of system: `/…` on
/// Windows, where it has no drive, and a drive's or a server's (`C:\…`,
/// `\\server\…`) elsewhere. It names no file here: its media are missing
/// until relinked (deviation X1), and it is never opened, as on Windows it
/// would name another file on this drive.
pub fn from_another_system(path: &Path) -> bool {
    path.to_str()
        .is_some_and(|s| absolute_elsewhere(s, cfg!(windows)))
}

/// [`from_another_system`] on Windows (`windows`), or on macOS and Linux.
pub(crate) fn absolute_elsewhere(s: &str, windows: bool) -> bool {
    if windows {
        s.starts_with('/') && !s.starts_with("//")
    } else {
        windows_absolute(s)
    }
}

/// A Windows absolute path: a drive's (`C:\…`, `C:/…`) or a server's
/// (`\\server\…`).
pub(crate) fn windows_absolute(s: &str) -> bool {
    s.starts_with("\\\\")
        || (s.get(..2).is_some_and(is_drive) && matches!(s.as_bytes().get(2), Some(b'\\' | b'/')))
}

/// A drive letter and colon (`C:`).
pub(crate) fn is_drive(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::model::Settings;
    use crate::request::Envelope;
    use crate::test_support::FakeEnv;

    /// An empty project in `dir`, its history there too.
    fn project_in(dir: &Path) -> Store {
        let path = dir.join("p.vidcut");
        let project = Project::new(Settings {
            auto_format: false,
            ..Settings::default()
        });
        std::fs::write(&path, to_json(&mut project.clone(), dir).unwrap()).unwrap();
        Store::open_in(&path, &dir.join("state")).unwrap()
    }

    /// Adds bin `name` to the project and saves it.
    fn add_bin(store: &Store, name: &str) {
        let mut editor = store.load().unwrap();
        let request = json!({"method": "bins.add", "params": {"path": name}});
        editor
            .apply(
                Envelope::from_json(request).unwrap(),
                &FakeEnv::default(),
                None,
            )
            .unwrap();
        store.save(&mut editor).unwrap();
    }

    fn history_text(store: &Store) -> String {
        std::fs::read_to_string(&store.history_path).unwrap()
    }

    #[test]
    fn a_saved_project_keeps_its_paths_absolute() {
        // The file has them relative to its folder.
        let tmp = tempfile::tempdir().unwrap();
        let dir = crate::dirs::canonical(tmp.path()).unwrap();
        let fixture = crate::test_support::Fixture::new(&[(4.0, true)]);
        let mut project = fixture.editor.project().clone();
        project.media[0].path = dir.join("1.mp4");
        let path = dir.join("p.vidcut");
        std::fs::write(&path, to_json(&mut project, &dir).unwrap()).unwrap();
        let store = Store::open_in(&path, &dir.join("state")).unwrap();
        let mut editor = store.load().unwrap();
        store.save(&mut editor).unwrap();
        assert_eq!(editor.project().media[0].path, dir.join("1.mp4"));
    }

    #[test]
    fn a_save_adds_lines_to_the_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = project_in(dir.path());
        add_bin(&store, "a");
        let before = history_text(&store);
        add_bin(&store, "b");
        let after = history_text(&store);
        assert!(after.len() > before.len() && after.starts_with(&before));
    }

    #[test]
    fn an_outside_edit_is_one_undo_step() {
        let dir = tempfile::tempdir().unwrap();
        let store = project_in(dir.path());
        add_bin(&store, "a");
        let text = store
            .read()
            .unwrap()
            .replace("\"ripple_on\": true", "\"ripple_on\": false");
        std::fs::write(store.path(), text).unwrap();
        let newest = store.load().unwrap().history_view(1).unwrap().undo;
        assert_eq!(newest[0].label, "external change");
    }

    #[test]
    fn a_cut_off_last_line_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let store = project_in(dir.path());
        add_bin(&store, "a");
        add_bin(&store, "b");
        let mut text = history_text(&store);
        text.push_str("{\"step\": {\"lab");
        std::fs::write(&store.history_path, text).unwrap();
        assert_eq!(store.load().unwrap().history_counts(), (2, 0));
    }

    #[test]
    fn a_name_starting_with_a_wide_character_is_relative() {
        assert!(!windows_absolute("日本/a.mp4"));
    }

    #[test]
    fn a_path_from_the_root_names_nothing_on_windows() {
        assert!(absolute_elsewhere("/Users/x/a.mp4", true));
        assert!(!absolute_elsewhere("//server/share/a.mp4", true));
        assert!(!absolute_elsewhere(r"C:\x\a.mp4", true));
    }

    #[test]
    fn a_drive_or_server_path_names_nothing_elsewhere() {
        assert!(absolute_elsewhere(r"C:\x\a.mp4", false));
        assert!(absolute_elsewhere(r"\\server\share\a.mp4", false));
        assert!(!absolute_elsewhere("/Users/x/a.mp4", false));
    }
}
