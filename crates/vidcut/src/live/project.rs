use std::path::Path;
use std::sync::mpsc::Sender;

use notify::Watcher;
use vidcut_core::model::Settings;
use vidcut_core::steps::{Steps, StepsCache};
use vidcut_core::{Editor, Env, Envelope, Project, Response, Result, Store};

use super::Msg;

pub struct OpenProject {
    pub store: Store,
    pub editor: Editor,
    disk: String,
    stamp: Option<(u64, std::time::SystemTime)>,
    pub history_error: Option<String>,
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
        let stamp = store.stamp();
        let disk = store.read()?;
        let opened = crate::when::now_ms();
        let mut editor = store.load_text(&disk)?;
        editor.opened_at(opened);
        let watcher = watch(store.path(), inbox, || Msg::FileChanged);
        Ok(Self {
            store,
            editor,
            disk,
            stamp,
            history_error: None,
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
        let _lock = self.store.lock()?;
        self.read_again()
    }

    pub fn refresh(&mut self) -> Result<bool> {
        let stamp = self.store.stamp();
        if stamp.is_some() && stamp == self.stamp {
            return Ok(false);
        }
        self.read_again()
    }

    fn read_again(&mut self) -> Result<bool> {
        let stamp = self.store.stamp();
        let text = self.store.read()?;
        self.stamp = stamp;
        if text == self.disk {
            return Ok(false);
        }
        let mut editor = self.store.load_text(&text)?;
        editor.opened_at(self.opened);
        let changed = editor.project() != self.editor.project()
            || editor.history_counts() != self.editor.history_counts();
        self.editor = editor;
        self.disk = text;
        Ok(changed)
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
        let _lock = match self.store.lock() {
            Ok(l) => l,
            Err(e) => return (Err(e), false),
        };
        let refreshed = match self.refresh() {
            Ok(r) => r,
            Err(e) => return (Err(e), false),
        };
        let rev = self.editor.project().rev;
        let result = self.editor.apply(envelope, env, playhead);
        let committed = self.editor.project().rev != rev;
        if committed {
            match self.store.save(&mut self.editor) {
                Ok(saved) => {
                    self.disk = saved.text;
                    self.stamp = self.store.stamp();
                    self.history_error = saved.history_error;
                    if saved.compact {
                        let store = self.store.clone();
                        let _ = crate::threads::spawn(
                            "vidcut-history",
                            crate::threads::Work::Job,
                            move || store.compact(),
                        );
                    }
                }
                Err(e) => return (Err(e), true),
            }
        }
        (result, committed || refreshed)
    }
}

pub(super) fn watch_dir(
    dir: &Path,
    inbox: Sender<Msg>,
    changed: fn() -> Msg,
) -> Option<notify::RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event
            && !matches!(event.kind, notify::EventKind::Access(_))
            && event
                .paths
                .iter()
                .any(|p| p.extension().is_some_and(|e| e == "toml"))
        {
            let _ = inbox.send(changed());
        }
    })
    .ok()?;
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
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event
            && !matches!(event.kind, notify::EventKind::Access(_))
            && event
                .paths
                .iter()
                .any(|p| p.file_name() == Some(name.as_os_str()))
        {
            let _ = inbox.send(changed());
        }
    })
    .ok()?;
    watcher
        .watch(&dir, notify::RecursiveMode::NonRecursive)
        .ok()?;
    Some(watcher)
}
