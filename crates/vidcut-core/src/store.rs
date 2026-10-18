use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::editor::{Editor, History};
use crate::error::{Error, Result};
use crate::history::{self, HistoryFile, Tail, text_hash};
use crate::model::Project;

const LOCK_TIMEOUT: Duration = Duration::from_secs(30);

pub fn state_dir() -> PathBuf {
    crate::dirs::state()
}

#[derive(Debug, Clone)]
pub struct Saved {
    pub text: String,
    pub history_error: Option<String>,
    pub compact: bool,
}

#[derive(Debug, Clone)]
pub struct Store {
    path: PathBuf,
    lock_path: PathBuf,
    history_path: PathBuf,
}

impl Store {
    pub fn create(path: &Path, project: &Project) -> Result<Self> {
        if path.exists() {
            return Err(Error::invalid(format!("{} already exists", path.display())));
        }
        Self::replace(path, project)
    }

    pub fn replace(path: &Path, project: &Project) -> Result<Self> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent).map_err(|e| io(path, e))?;
        let dir = crate::dirs::canonical(parent).map_err(|e| io(path, e))?;
        write_atomic(path, &to_json(&mut project.clone(), &dir)?).map_err(|e| io(path, e))?;
        let store = Self::open(path)?;
        let _ = std::fs::remove_file(&store.history_path);
        Ok(store)
    }

    pub fn open(path: &Path) -> Result<Self> {
        Self::open_in(path, &state_dir())
    }

    pub(crate) fn open_in(path: &Path, state: &Path) -> Result<Self> {
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
        let name = crate::fingerprint::short_digest(path.as_os_str().as_encoded_bytes());
        let history_path = state.join("history").join(format!("{name}.jsonl"));
        Ok(Self {
            path,
            lock_path,
            history_path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn history_path(&self) -> &Path {
        &self.history_path
    }

    fn dir(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new("/"))
    }

    pub fn stamp(&self) -> Option<(u64, std::time::SystemTime)> {
        let meta = std::fs::metadata(&self.path).ok()?;
        Some((meta.len(), meta.modified().ok()?))
    }

    pub fn read(&self) -> Result<String> {
        std::fs::read_to_string(&self.path).map_err(|e| io(&self.path, e))
    }

    pub fn load(&self) -> Result<Editor> {
        self.load_text(&self.read()?)
    }

    pub fn load_text(&self, text: &str) -> Result<Editor> {
        let project = parse(&self.path, self.dir(), text)?;
        let file = self.history_file();
        let hash = text_hash(text);
        let (tail, end) = match settled(&file) {
            Ok(Some(found)) => found,
            Ok(None) => return Ok(Editor::new(project, History::default())),
            Err(e) => return Ok(Editor::unreadable(project, hash, self.trouble(&e))),
        };
        if tail.hash == hash {
            return Ok(Editor::on_file(project, file, hash));
        }
        let Some(last) = last_saved(&file, end) else {
            let why = "its last saved project can't be rebuilt from it".to_string();
            return Ok(Editor::unreadable(project, hash, self.trouble(&why)));
        };
        let mut editor = Editor::on_file(last, file, tail.hash);
        editor.record_external(project);
        Ok(editor)
    }

    fn trouble(&self, why: &dyn std::fmt::Display) -> String {
        format!(
            "the undo history {} can't be read ({why}). This session's steps are kept in \
             memory and the file is left as it is",
            self.history_path.display()
        )
    }

    pub fn project_at(
        &self,
        past: &mut crate::past::Past,
        current: Project,
        step: usize,
    ) -> Result<std::sync::Arc<Project>> {
        past.project_at(
            &Editor::on_file(current, self.history_file(), String::new()),
            step,
        )
    }

    pub fn recover(&self, editor: &mut Editor) -> bool {
        let Some(hash) = editor.blocked_on().map(String::from) else {
            return false;
        };
        if editor.pending() {
            return false;
        }
        let file = self.history_file();
        match file.read_tail() {
            Ok(Some((tail, _))) if tail.hash == hash => {
                editor.saved_to(file, hash);
                true
            }
            _ => false,
        }
    }

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

    pub fn transact<T>(
        &self,
        f: impl FnOnce(&mut Editor) -> Result<T>,
    ) -> Result<(T, Option<String>)> {
        let lock = self.lock()?;
        let mut editor = self.load()?;
        let rev = editor.project().rev;
        let out = f(&mut editor)?;
        if editor.project().rev == rev && !editor.pending() {
            return Ok((out, None));
        }
        let saved = self.save(&mut editor)?;
        drop(lock);
        if saved.compact {
            let _ = self.compact();
        }
        Ok((out, saved.history_error))
    }

    pub fn save(&self, editor: &mut Editor) -> Result<Saved> {
        let text = to_json(editor.project_mut(), self.dir())?;
        let history = self.save_history(editor, &text_hash(&text));
        write_atomic(&self.path, &text).map_err(|e| io(&self.path, e))?;
        Ok(Saved {
            text,
            compact: history.as_ref().is_ok_and(history::due),
            history_error: history
                .err()
                .map(|e| format!("{}: {e}", self.history_path.display())),
        })
    }

    fn save_history(&self, editor: &mut Editor, hash: &str) -> std::io::Result<Tail> {
        let file = self.history_file();
        let found = settled(&file);
        let stands_on = editor.stands_on().map(String::from);
        match found {
            Ok(Some((tail, end))) if Some(&tail.hash) == stands_on.as_ref() => {
                let moved = editor.take_moved();
                let tail = file.append(&tail, end, moved, hash)?;
                editor.saved_to(file, hash.into());
                return Ok(tail);
            }
            Ok(None) => {}
            Ok(Some(_)) => {
                let why = "another program changed it since this session read it";
                editor.block(self.trouble(&why));
                return Err(std::io::Error::other(why));
            }
            Err(e) => {
                editor.block(self.trouble(&e));
                return Err(e);
            }
        }
        editor.take_moved();
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

    pub fn compact(&self) -> Result<()> {
        self.history_file()
            .compact(|| {
                self.lock()
                    .map_err(|e| std::io::Error::other(e.to_string()))
            })
            .map_err(|e| io(&self.history_path, e))
    }

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

fn settled(file: &HistoryFile) -> std::io::Result<Option<(Tail, u64)>> {
    let mut tries = 0;
    loop {
        match file.read_tail() {
            Err(_) if tries < 10 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
            found => return found,
        }
    }
}

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

fn files(project: &mut Project) -> impl Iterator<Item = &mut PathBuf> {
    project.media.iter_mut().map(|m| &mut m.path)
}

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

fn portable(relative: &Path) -> PathBuf {
    #[cfg(windows)]
    let path = PathBuf::from(relative.to_string_lossy().replace('\\', "/"));
    #[cfg(not(windows))]
    let path = relative.to_path_buf();
    path
}

fn native(relative: &Path) -> PathBuf {
    #[cfg(windows)]
    let path = PathBuf::from(relative.to_string_lossy().replace('/', "\\"));
    #[cfg(not(windows))]
    let path = relative.to_path_buf();
    path
}

fn absolute_anywhere(path: &Path) -> bool {
    let Some(s) = path.to_str() else {
        return path.is_absolute();
    };
    s.starts_with(['/', '\\']) || windows_absolute(s)
}

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

pub fn from_another_system(path: &Path) -> bool {
    path.to_str()
        .is_some_and(|s| absolute_elsewhere(s, cfg!(windows)))
}

pub(crate) fn absolute_elsewhere(s: &str, windows: bool) -> bool {
    if windows {
        s.starts_with('/') && !s.starts_with("//")
    } else {
        windows_absolute(s)
    }
}

pub(crate) fn windows_absolute(s: &str) -> bool {
    s.starts_with("\\\\")
        || (s.get(..2).is_some_and(is_drive) && matches!(s.as_bytes().get(2), Some(b'\\' | b'/')))
}

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

    fn project_in(dir: &Path) -> Store {
        let path = dir.join("p.vidcut");
        let project = Project::new(Settings {
            auto_format: false,
            ..Settings::default()
        });
        std::fs::write(&path, to_json(&mut project.clone(), dir).unwrap()).unwrap();
        Store::open_in(&path, &dir.join("state")).unwrap()
    }

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
        assert_eq!(newest[0].step.label, crate::editor::EXTERNAL);
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

    fn break_history(store: &Store) -> String {
        let good = history_text(store);
        std::fs::write(&store.history_path, format!("x{good}")).unwrap();
        good
    }

    #[test]
    fn a_history_that_cant_be_read_isnt_written_over() {
        let dir = tempfile::tempdir().unwrap();
        let store = project_in(dir.path());
        add_bin(&store, "a");
        let good = break_history(&store);
        add_bin(&store, "b");
        assert_eq!(history_text(&store), format!("x{good}"));
    }

    #[test]
    fn a_history_that_cant_be_read_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let store = project_in(dir.path());
        add_bin(&store, "a");
        break_history(&store);
        let editor = store.load().unwrap();
        assert!(
            editor
                .history_trouble()
                .is_some_and(|t| t.contains("can't be read"))
        );
    }

    #[test]
    fn steps_made_while_the_history_was_unreadable_join_it_when_it_reads_again() {
        let dir = tempfile::tempdir().unwrap();
        let store = project_in(dir.path());
        add_bin(&store, "a");
        let good = break_history(&store);
        let mut editor = store.load().unwrap();
        let request = json!({"method": "bins.add", "params": {"path": "b"}});
        editor
            .apply(
                Envelope::from_json(request).unwrap(),
                &FakeEnv::default(),
                None,
            )
            .unwrap();
        assert!(store.save(&mut editor).unwrap().history_error.is_some());
        std::fs::write(&store.history_path, good).unwrap();
        let request = json!({"method": "bins.add", "params": {"path": "c"}});
        editor
            .apply(
                Envelope::from_json(request).unwrap(),
                &FakeEnv::default(),
                None,
            )
            .unwrap();
        store.save(&mut editor).unwrap();
        let labels: Vec<String> = store
            .load()
            .unwrap()
            .history_view(3)
            .unwrap()
            .undo
            .into_iter()
            .map(|e| e.step.label)
            .collect();
        assert_eq!(labels, ["New bin a", "New bin b", "New bin c"]);
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
