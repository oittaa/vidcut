use std::path::Path;
use std::sync::mpsc::Sender;

use notify::Watcher;
use vidcut_core::model::Settings;
use vidcut_core::steps::{Steps, StepsCache};
use vidcut_core::store::Saved;
use vidcut_core::{Editor, Env, Envelope, Project, Response, Result, Store};

use super::Msg;

pub struct OpenProject {
    pub store: Store,
    pub editor: Editor,
    disk: String,
    stamp: Option<(u64, std::time::SystemTime)>,
    pub unsaved: Option<String>,
    pub save_failed: bool,
    told: Option<String>,
    opened: u64,
    steps: StepsCache,
    history_stamp: Option<(u64, std::time::SystemTime)>,
    _watcher: Option<notify::RecommendedWatcher>,
    _history_watcher: Option<notify::RecommendedWatcher>,
}

pub struct New {
    pub settings: Settings,
    pub replace: bool,
}

impl OpenProject {
    pub fn open(path: &Path, create: Option<New>, inbox: Sender<Msg>) -> Result<Self> {
        if let Some(new) = create {
            let project = Project::new(new.settings);
            if new.replace {
                Store::replace(path, &project)?;
            } else {
                Store::create(path, &project)?;
            }
        }
        let store = Store::open(path)?;
        let stamp = store.stamp();
        let disk = store.read()?;
        let opened = crate::when::now_ms();
        let mut editor = store.load_text(&disk)?;
        editor.opened_at(opened);
        let history_stamp = store.history_stamp();
        let history_watcher = watch(store.history_path(), inbox.clone(), || Msg::FileChanged);
        let watcher = watch(store.path(), inbox, || Msg::FileChanged);
        Ok(Self {
            store,
            editor,
            disk,
            stamp,
            unsaved: None,
            save_failed: false,
            told: None,
            opened,
            steps: StepsCache::default(),
            history_stamp,
            _watcher: watcher,
            _history_watcher: history_watcher,
        })
    }

    pub fn project(&self) -> &Project {
        self.editor.project()
    }

    pub fn trouble(&mut self) -> Option<String> {
        let now = self.editor.history_trouble().map(String::from);
        if now == self.told {
            return None;
        }
        self.told.clone_from(&now);
        now
    }

    pub fn steps(&mut self) -> Result<Steps> {
        self.store.recover(&mut self.editor);
        self.editor.history_steps(&mut self.steps)
    }

    pub fn reload(&mut self) -> Result<bool> {
        let _lock = self.store.lock()?;
        self.read_again()
    }

    pub fn refresh(&mut self) -> Result<bool> {
        let stamp = self.store.stamp();
        if stamp.is_some()
            && stamp == self.stamp
            && self.store.history_stamp() == self.history_stamp
        {
            return Ok(false);
        }
        self.read_again()
    }

    fn read_again(&mut self) -> Result<bool> {
        let stamp = self.store.stamp();
        let history = self.store.history_stamp();
        let text = self.store.read()?;
        self.stamp = stamp;
        let history_moved = history != self.history_stamp;
        self.history_stamp = history;
        if text == self.disk {
            return Ok(history_moved);
        }
        let mut editor = self.store.load_text(&text)?;
        editor.opened_at(self.opened);
        let changed = editor.project() != self.editor.project()
            || editor.history_counts() != self.editor.history_counts();
        self.editor = editor;
        self.disk = text;
        self.save_failed = false;
        Ok(changed)
    }

    pub fn apply(
        &mut self,
        envelope: Envelope,
        env: &dyn Env,
        playhead: Option<i64>,
    ) -> (Result<Response>, bool) {
        self.store.recover(&mut self.editor);
        if !envelope.request.is_mutation() {
            let refreshed = self.refresh().unwrap_or(false);
            return (self.editor.apply(envelope, env, playhead), refreshed);
        }
        let _lock = match self.store.lock() {
            Ok(l) => l,
            Err(e) => return (Err(e), false),
        };
        let refreshed = match self.refresh() {
            Ok(r) => r,
            Err(e) => return (Err(e), false),
        };
        let was = self.editor.unsaved();
        let result = self.editor.apply(envelope, env, playhead);
        let committed = self.editor.unsaved() != was;
        if committed {
            let saved = self.store.save(&mut self.editor);
            self.unsaved = unsaved_warning(&saved);
            self.save_failed = saved.is_err();
            if let Ok(saved) = saved {
                self.disk = saved.text;
                self.stamp = self.store.stamp();
                self.history_stamp = self.store.history_stamp();
                if saved.compact {
                    let store = self.store.clone();
                    let _ = crate::threads::spawn(
                        "vidcut-history",
                        crate::threads::Work::Job,
                        move || store.compact(),
                    );
                }
            }
        }
        (result, committed || refreshed)
    }

    pub fn save_left(&mut self) {
        if !self.save_failed {
            return;
        }
        let Ok(_lock) = self.store.lock() else {
            return;
        };
        if self.store.stamp() == self.stamp
            && let Ok(saved) = self.store.save(&mut self.editor)
        {
            self.disk = saved.text;
            self.stamp = self.store.stamp();
            self.history_stamp = self.store.history_stamp();
            self.save_failed = false;
        }
    }
}

fn unsaved_warning(saved: &Result<Saved>) -> Option<String> {
    match saved {
        Ok(saved) => saved
            .history_error
            .as_deref()
            .map(crate::session::history_unsaved),
        Err(e) => Some(format!(
            "the edit isn't saved ({}); the next edit tries again",
            e.message
        )),
    }
}

pub(super) fn watcher(
    relevant: impl Fn(&Path) -> bool + Send + 'static,
    inbox: Sender<Msg>,
    changed: fn() -> Msg,
) -> Option<notify::RecommendedWatcher> {
    notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event
            && !matches!(event.kind, notify::EventKind::Access(_))
            && event.paths.iter().any(|p| relevant(p))
        {
            let _ = inbox.send(changed());
        }
    })
    .ok()
}

pub(super) fn watch_dir(
    dir: &Path,
    inbox: Sender<Msg>,
    changed: fn() -> Msg,
) -> Option<notify::RecommendedWatcher> {
    let mut watcher = watcher(
        |p| p.extension().is_some_and(|e| e == "toml"),
        inbox,
        changed,
    )?;
    watcher
        .watch(dir, notify::RecursiveMode::NonRecursive)
        .ok()?;
    Some(watcher)
}

pub(super) fn watch(
    path: &Path,
    inbox: Sender<Msg>,
    changed: fn() -> Msg,
) -> Option<notify::RecommendedWatcher> {
    let dir = path.parent()?.to_path_buf();
    let name = path.file_name()?.to_os_string();
    let mut watcher = watcher(
        move |p| p.file_name() == Some(name.as_os_str()),
        inbox,
        changed,
    )?;
    watcher
        .watch(&dir, notify::RecursiveMode::NonRecursive)
        .ok()?;
    Some(watcher)
}

#[cfg(test)]
mod tests {
    use vidcut_core::Error;

    use super::*;

    fn saved(history_error: Option<&str>) -> Result<Saved> {
        Ok(Saved {
            text: String::new(),
            history_error: history_error.map(String::from),
            compact: false,
        })
    }

    #[test]
    fn a_failed_write_says_the_edit_is_made_but_not_saved() {
        assert_eq!(
            unsaved_warning(&Err(Error::internal("disk full"))).as_deref(),
            Some("the edit isn't saved (disk full); the next edit tries again")
        );
    }

    #[test]
    fn a_clean_save_has_no_warning() {
        assert_eq!(unsaved_warning(&saved(None)), None);
    }

    #[test]
    fn a_save_without_its_history_says_so() {
        let warning = unsaved_warning(&saved(Some("no room"))).unwrap();
        assert!(warning.contains("undo history isn't (no room)"));
    }

    fn opened(dir: &Path) -> OpenProject {
        let (inbox, _) = std::sync::mpsc::channel();
        let new = New {
            settings: Settings::default(),
            replace: false,
        };
        OpenProject::open(&dir.join("p.vidcut"), Some(new), inbox).unwrap()
    }

    #[test]
    fn a_save_that_is_left_is_written_while_the_file_is_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let mut open = opened(dir.path());
        open.save_failed = true;
        open.save_left();
        assert!(!open.save_failed);
    }

    #[test]
    fn a_save_that_is_left_waits_while_the_file_is_another() {
        let dir = tempfile::tempdir().unwrap();
        let mut open = opened(dir.path());
        open.save_failed = true;
        std::fs::write(open.store.path(), "{}").unwrap();
        open.save_left();
        assert!(open.save_failed);
    }
}
