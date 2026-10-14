//! Project files on disk: atomic writes, a per-project lock, and the undo
//! history sidecar that lets `vidcut history undo` work across CLI runs (deviation X4).
//!
//! The project is one pretty-printed JSON file. History lives outside it, in
//! `$XDG_STATE_HOME/vidcut/history/<hash of the project path>.jsonl`, one
//! JSON line each ([`Line`]): the project file it is of, the project before
//! its first step, then the steps (what each edit changed, `crate::change`),
//! undos and redos as they happened, each save ending with the hash of the
//! project text it wrote. A save adds lines at the end. The file is written
//! whole when it is new, when another process wrote it last, or when it
//! holds twice [`HISTORY_CAP`] steps. If the project file's hash isn't the
//! last one saved, someone edited it outside: the difference becomes one
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
use serde_json::Value;

use crate::editor::{Editor, Entry, HISTORY_CAP, History, Moved};
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

/// A line of the history file.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Line {
    /// The first: the project file the history is of.
    Of(PathBuf),
    /// The second: the project before the first step.
    Base(Value),
    Step(Entry),
    Undo(u32),
    Redo(u32),
    /// The hash of the project text a save wrote ([`text_hash`]).
    Saved(String),
}

impl From<Moved> for Line {
    fn from(m: Moved) -> Self {
        match m {
            Moved::Step(e) => Self::Step(e),
            Moved::Undo(n) => Self::Undo(n),
            Moved::Redo(n) => Self::Redo(n),
        }
    }
}

/// The history file as an editor last read or wrote it: its length, and
/// the steps in it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct OnDisk {
    len: u64,
    steps: usize,
}

/// What a history file holds.
struct Read {
    /// Its second line, the project before the first step, as written.
    base: String,
    /// Every step in it, not only the newest [`HISTORY_CAP`].
    history: History,
    /// The hash the last save wrote.
    saved: Option<String>,
    /// Where it can take more lines: `None` when its last line is cut off.
    on_disk: Option<OnDisk>,
}

impl Read {
    /// The project as the history's last save left it: its first project
    /// with every step made again. `None` when they don't fit.
    fn last_saved(&self) -> Option<Project> {
        let Ok(Line::Base(mut json)) = serde_json::from_str(&self.base) else {
            return None;
        };
        for e in &self.history.undo {
            e.change.apply(&mut json, true).ok()?;
        }
        serde_json::from_value(json).ok()
    }
}

/// The hash of a project file's text, as the history keeps it.
fn text_hash(text: &str) -> String {
    format!("{:016x}", fnv1a(text.as_bytes()))
}

fn line(l: &Line) -> String {
    let mut s = serde_json::to_string(l).expect("history lines are JSON");
    s.push('\n');
    s
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
        let hash = fnv1a(path.as_os_str().as_encoded_bytes());
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
        let Some(read) = self.read_history() else {
            return Ok(Editor::new(project, History::default()));
        };
        if read.saved.as_deref() == Some(text_hash(text).as_str()) {
            let mut editor = Editor::new(project, read.history);
            editor.on_disk = read.on_disk;
            return Ok(editor);
        }
        // Edited outside: the project as the history left it, and then the
        // file as it is now, one step.
        let Some(last) = read.last_saved() else {
            return Ok(Editor::new(project, History::default()));
        };
        let mut editor = Editor::new(last, read.history);
        editor.on_disk = read.on_disk;
        editor.record_external(project);
        Ok(editor)
    }

    /// The history file, when it is this project's and whole but for maybe
    /// its last line.
    fn read_history(&self) -> Option<Read> {
        let text = std::fs::read_to_string(&self.history_path).ok()?;
        let mut lines = text.split_inclusive('\n');
        match serde_json::from_str(lines.next()?).ok()? {
            Line::Of(path) if path == self.path => {}
            _ => return None,
        }
        let mut read = Read {
            base: lines.next()?.to_string(),
            history: History::default(),
            saved: None,
            on_disk: None,
        };
        let (mut steps, mut whole) = (0, true);
        for l in lines {
            if !whole {
                return None;
            }
            let Ok(l) = serde_json::from_str::<Line>(l) else {
                // Cut off while it was written: the lines before it stand.
                whole = false;
                continue;
            };
            let h = &mut read.history;
            match l {
                Line::Step(e) => {
                    h.undo.push(e);
                    h.redo.clear();
                    steps += 1;
                }
                Line::Undo(n) => shift(n, &mut h.undo, &mut h.redo)?,
                Line::Redo(n) => shift(n, &mut h.redo, &mut h.undo)?,
                Line::Saved(hash) => read.saved = Some(hash),
                Line::Of(_) | Line::Base(_) => return None,
            }
        }
        read.on_disk = whole.then_some(OnDisk {
            len: text.len() as u64,
            steps,
        });
        Some(read)
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
            self.save(&mut editor)?.history_error
        } else {
            None
        };
        Ok((out, history_error))
    }

    /// Writes the project and its history. Callers hold the
    /// [`lock`](Self::lock).
    pub fn save(&self, editor: &mut Editor) -> Result<Saved> {
        let text = to_json(editor.project(), self.dir())?;
        // The history goes first: whoever sees the new project file (a file
        // watcher, say) then also finds the history that matches it, instead
        // of mistaking the change for an external edit. Failing to save the
        // history must not lose the edit: the next load sees it as one.
        let history_error = self
            .save_history(editor, &text_hash(&text))
            .err()
            .map(|e| format!("{}: {e}", self.history_path.display()));
        write_atomic(&self.path, &text).map_err(|e| io(&self.path, e))?;
        Ok(Saved {
            text,
            history_error,
        })
    }

    /// Adds what happened to `editor`'s history since it was last saved to
    /// the history file, or writes the file whole.
    fn save_history(&self, editor: &mut Editor, hash: &str) -> std::io::Result<()> {
        let moved = editor.take_moved();
        let steps = moved.iter().filter(|m| matches!(m, Moved::Step(_))).count();
        let len = std::fs::metadata(&self.history_path).map(|m| m.len()).ok();
        // Until it is written, where the file stands isn't known.
        let on_disk = editor.on_disk.take();
        if let Some(d) = on_disk
            && Some(d.len) == len
            && d.steps + steps <= 2 * HISTORY_CAP
        {
            let mut add: String = moved.into_iter().map(|m| line(&m.into())).collect();
            add.push_str(&line(&Line::Saved(hash.into())));
            File::options()
                .append(true)
                .open(&self.history_path)?
                .write_all(add.as_bytes())?;
            editor.on_disk = Some(OnDisk {
                len: d.len + add.len() as u64,
                steps: d.steps + steps,
            });
            return Ok(());
        }
        let h = editor.history();
        let mut base = serde_json::to_value(editor.project()).map_err(std::io::Error::other)?;
        for e in h.undo.iter().rev() {
            e.change
                .apply(&mut base, false)
                .map_err(std::io::Error::other)?;
        }
        let mut text = line(&Line::Of(self.path.clone()));
        text.push_str(&line(&Line::Base(base)));
        // The steps to redo as if made and then undone.
        for e in h.undo.iter().chain(h.redo.iter().rev()) {
            text.push_str(&line(&Line::Step(e.clone())));
        }
        if !h.redo.is_empty() {
            text.push_str(&line(&Line::Undo(h.redo.len() as u32)));
        }
        text.push_str(&line(&Line::Saved(hash.into())));
        let steps = h.undo.len() + h.redo.len();
        std::fs::create_dir_all(self.history_path.parent().expect("history dir"))?;
        write_atomic(&self.history_path, &text)?;
        editor.on_disk = Some(OnDisk {
            len: text.len() as u64,
            steps,
        });
        Ok(())
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

/// Moves the newest `n` steps from one stack to the other.
fn shift(n: u32, from: &mut Vec<Entry>, to: &mut Vec<Entry>) -> Option<()> {
    for _ in 0..n {
        to.push(from.pop()?);
    }
    Some(())
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
        std::fs::write(&path, to_json(&project, dir).unwrap()).unwrap();
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
        let editor = store.load().unwrap();
        let labels: Vec<&str> = editor
            .history()
            .undo
            .iter()
            .map(|e| e.label.as_str())
            .collect();
        assert_eq!(labels.last(), Some(&"external change"));
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
        assert_eq!(store.load().unwrap().history().undo.len(), 2);
    }

    #[test]
    fn past_twice_the_limit_the_history_is_written_whole() {
        let dir = tempfile::tempdir().unwrap();
        let store = project_in(dir.path());
        for i in 0..=2 * HISTORY_CAP {
            add_bin(&store, &format!("b{i}"));
        }
        let steps = history_text(&store)
            .lines()
            .filter(|l| l.starts_with("{\"step\""))
            .count();
        assert_eq!(steps, HISTORY_CAP);
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
