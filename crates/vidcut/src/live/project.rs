use std::path::Path;
use std::sync::mpsc::Sender;

use notify::Watcher;
use vidcut_core::model::Settings;
use vidcut_core::steps::{Steps, StepsCache};
use vidcut_core::store::{FileId, Saved};
use vidcut_core::{Editor, Env, Envelope, Project, Response, Result, Store};

use super::Msg;

pub struct OpenProject {
    pub store: Store,
    pub editor: Editor,
    file_id: Option<FileId>,
    data_version: i64,
    pub unsaved: Option<String>,
    pub save_failed: bool,
    opened: u64,
    steps: StepsCache,
    _watcher: Option<notify::RecommendedWatcher>,
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
        let opened = crate::when::now_ms();
        let mut editor = store.load()?;
        editor.opened_at(opened);
        let file_id = store.file_id();
        let data_version = store.data_version()?;
        let watcher = watch_project(store.path(), inbox, || Msg::FileChanged);
        Ok(Self {
            store,
            editor,
            file_id,
            data_version,
            unsaved: None,
            save_failed: false,
            opened,
            steps: StepsCache::default(),
            _watcher: watcher,
        })
    }

    pub fn project(&self) -> &Project {
        self.editor.project()
    }

    pub fn steps(&mut self) -> Result<Steps> {
        self.editor.history_steps(&mut self.steps)
    }

    pub fn reload(&mut self) -> Result<bool> {
        self.refresh()
    }

    pub fn refresh(&mut self) -> Result<bool> {
        let Some(mut editor) = self
            .store
            .reload_if_changed(&mut self.file_id, &mut self.data_version)?
        else {
            return Ok(false);
        };
        editor.opened_at(self.opened);
        self.editor = editor;
        self.save_failed = false;
        Ok(true)
    }

    pub fn apply(
        &mut self,
        envelope: Envelope,
        env: &dyn Env,
        playhead: Option<i64>,
    ) -> (Result<Response>, bool) {
        if !envelope.request.is_mutation() {
            let refreshed = self.refresh().unwrap_or(false);
            return (self.editor.apply(envelope, env, playhead), refreshed);
        }
        let outcome = match self.store.mutate_live(
            &mut self.file_id,
            &mut self.data_version,
            &mut self.editor,
            self.opened,
            |editor| editor.apply(envelope, env, playhead),
        ) {
            Ok(outcome) => outcome,
            Err(e) => return (Err(e), false),
        };
        if outcome.committed {
            let saved = outcome.saved.expect("committed save result");
            self.unsaved = unsaved_warning(&saved);
            self.save_failed = saved.is_err();
            if let Ok(saved) = saved
                && saved.compact
            {
                let store = self.store.clone();
                let _ =
                    crate::threads::spawn("vidcut-history", crate::threads::Work::Job, move || {
                        store.compact()
                    });
            }
        }
        (outcome.result, outcome.committed || outcome.refreshed)
    }

    pub fn save_left(&mut self) {
        if !self.save_failed {
            return;
        }
        if self
            .store
            .save_if_unchanged(&mut self.file_id, &mut self.data_version, &mut self.editor)
            .is_ok_and(|saved| saved)
        {
            self.save_failed = false;
        }
    }

    pub fn close(&mut self) {
        self.save_left();
        self.editor.close_db();
        self.store.checkpoint();
    }
}

fn unsaved_warning(saved: &Result<Saved>) -> Option<String> {
    match saved {
        Ok(_) => None,
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

fn watch_project(
    path: &Path,
    inbox: Sender<Msg>,
    changed: fn() -> Msg,
) -> Option<notify::RecommendedWatcher> {
    let dir = path.parent()?.to_path_buf();
    let name = path.file_name()?.to_os_string();
    let mut wal = name.clone();
    wal.push("-wal");
    let mut journal = name.clone();
    journal.push("-journal");
    let mut watcher = watcher(
        move |p| {
            p.file_name().is_some_and(|f| {
                f == name.as_os_str() || f == wal.as_os_str() || f == journal.as_os_str()
            })
        },
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

    fn saved() -> Result<Saved> {
        Ok(Saved { compact: false })
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
        assert_eq!(unsaved_warning(&saved()), None);
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
    fn a_save_that_is_left_waits_while_the_file_is_another() {
        let dir = tempfile::tempdir().unwrap();
        let mut open = opened(dir.path());
        open.save_failed = true;
        std::fs::write(open.store.path(), "{}").unwrap();
        open.save_left();
        assert!(open.save_failed);
    }

    #[test]
    fn reload_after_own_save_returns_false() {
        let dir = tempfile::tempdir().unwrap();
        let mut open = opened(dir.path());
        let env = crate::env::AvEnv::for_project(open.store.path());
        let add =
            Envelope::from_json(serde_json::json!({"method": "bins.add", "params": {"path": "a"}}))
                .unwrap();
        let (res, _) = open.apply(add, &env, None);
        res.unwrap();
        assert!(!open.reload().unwrap());
    }

    #[test]
    fn reload_sees_external_commit() {
        let dir = tempfile::tempdir().unwrap();
        let mut open = opened(dir.path());
        let env = crate::env::AvEnv::for_project(open.store.path());
        let ext = Store::open(open.store.path()).unwrap();
        let add_b =
            Envelope::from_json(serde_json::json!({"method": "bins.add", "params": {"path": "b"}}))
                .unwrap();
        ext.transact(|ed| ed.apply(add_b, &env, None)).unwrap();
        assert!(open.reload().unwrap());
        assert_eq!(open.project().bins, vec!["b"]);
    }

    #[test]
    fn close_releases_history_handle_so_replace_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let mut open = opened(dir.path());
        let env = crate::env::AvEnv::for_project(open.store.path());
        let add =
            Envelope::from_json(serde_json::json!({"method": "bins.add", "params": {"path": "a"}}))
                .unwrap();
        open.apply(add, &env, None).0.unwrap();
        let _ = open.steps().unwrap();
        let path = open.store.path().to_path_buf();
        open.close();
        let (inbox, _) = std::sync::mpsc::channel();
        let replaced = OpenProject::open(
            &path,
            Some(New {
                settings: Settings::default(),
                replace: true,
            }),
            inbox,
        )
        .unwrap();
        assert!(replaced.project().bins.is_empty());
    }

    #[test]
    fn save_left_skips_when_another_writer_committed() {
        let dir = tempfile::tempdir().unwrap();
        let mut open = opened(dir.path());
        let env = crate::env::AvEnv::for_project(open.store.path());
        let add_a =
            Envelope::from_json(serde_json::json!({"method": "bins.add", "params": {"path": "a"}}))
                .unwrap();
        open.editor.apply(add_a, &env, None).unwrap();
        open.save_failed = true;
        let ext = Store::open(open.store.path()).unwrap();
        let add_b =
            Envelope::from_json(serde_json::json!({"method": "bins.add", "params": {"path": "b"}}))
                .unwrap();
        ext.transact(|ed| ed.apply(add_b, &env, None)).unwrap();
        open.save_left();
        assert!(open.save_failed);
        assert_eq!(ext.load().unwrap().project().bins, vec!["b"]);
    }
}
