//! The open project, kept in step with its file.
//!
//! The file is the source of truth (deviation X1): every committed change is
//! written before the reply, under the same lock headless CLI runs take, and
//! changes made by other processes (a headless CLI run, an editor) are read
//! back when the file watcher reports them, or at the latest before the next
//! change, so nothing is ever overwritten blindly.

use std::path::Path;
use std::sync::mpsc::Sender;

use notify::Watcher;
use vidcut_core::model::Settings;
use vidcut_core::{Editor, Env, Envelope, Project, Response, Result, Store};

use super::Msg;

pub struct OpenProject {
    pub store: Store,
    pub editor: Editor,
    /// The file's text when last read or written.
    disk: String,
    /// The file's [`Store::stamp`] then.
    stamp: Option<(u64, std::time::SystemTime)>,
    /// Why the undo history couldn't be saved with the latest edit, until
    /// the session shows it.
    pub history_error: Option<String>,
    _watcher: Option<notify::RecommendedWatcher>,
}

/// A project to make before opening it.
pub struct New {
    pub settings: Settings,
    /// Over a file already there.
    pub replace: bool,
}

impl OpenProject {
    /// Opens `path`, first making `create` there, and watches it for
    /// outside changes.
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
        // Before the read: a write after it changes the stamp again.
        let stamp = store.stamp();
        let disk = store.read()?;
        let editor = store.load_text(&disk)?;
        let watcher = watch(store.path(), inbox, || Msg::FileChanged);
        Ok(Self {
            store,
            editor,
            disk,
            stamp,
            history_error: None,
            _watcher: watcher,
        })
    }

    pub fn project(&self) -> &Project {
        self.editor.project()
    }

    /// Re-reads the file under the project lock (so a writer's project file
    /// and history are seen together); returns whether the project changed.
    /// The watcher said it changed, so it is read whatever its stamp says.
    pub fn reload(&mut self) -> Result<bool> {
        let _lock = self.store.lock()?;
        self.read_again()
    }

    /// Re-reads the file if its stamp changed; returns whether the project
    /// changed. Callers that don't hold the lock should use
    /// [`reload`](Self::reload).
    pub fn refresh(&mut self) -> Result<bool> {
        let stamp = self.store.stamp();
        if stamp.is_some() && stamp == self.stamp {
            return Ok(false);
        }
        self.read_again()
    }

    /// Reads the file and takes it in if it changed.
    fn read_again(&mut self) -> Result<bool> {
        let stamp = self.store.stamp();
        let text = self.store.read()?;
        self.stamp = stamp;
        if text == self.disk {
            return Ok(false);
        }
        // `load_text` takes the history from the sidecar a headless run
        // wrote, or records an "external change" undo step.
        let editor = self.store.load_text(&text)?;
        let changed = editor.project() != self.editor.project()
            || editor.history_counts() != self.editor.history_counts();
        self.editor = editor;
        self.disk = text;
        Ok(changed)
    }

    /// Applies a request; mutations are saved under the project lock.
    /// Returns the response and whether the project changed (possibly by an
    /// outside change picked up on the way, even if the request failed).
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
                        // Trimming the history writes it anew: on a thread
                        // of its own, the lock taken only to swap the files.
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

/// Watches the folder `dir` and reports changes to the TOML files in it
/// with the message `changed` makes.
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

/// Watches the directory of the file at `path` and reports changes to the
/// file with the message `changed` makes.
pub(super) fn watch(
    path: &Path,
    inbox: Sender<Msg>,
    changed: fn() -> Msg,
) -> Option<notify::RecommendedWatcher> {
    let dir = path.parent()?.to_path_buf();
    let name = path.file_name()?.to_os_string();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        // Reading the file reports access events: reacting to them would
        // re-read the file forever.
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
