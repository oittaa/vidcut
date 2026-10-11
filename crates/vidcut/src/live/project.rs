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
use vidcut_core::store::HistoryError;
use vidcut_core::{Editor, Env, Envelope, Project, Response, Result, Store};

use super::Msg;

pub struct OpenProject {
    pub store: Store,
    pub editor: Editor,
    /// The file's text when last read or written.
    disk: String,
    /// Why the undo history couldn't be saved with the latest edit, until
    /// the session reports it.
    pub history_error: Option<HistoryError>,
    _watcher: Option<notify::RecommendedWatcher>,
}

impl OpenProject {
    /// Opens `path` (creating an empty project there if `create` and it is
    /// missing) and watches it for outside changes.
    pub fn open(path: &Path, create: bool, inbox: Sender<Msg>) -> Result<Self> {
        if create && !path.exists() {
            Store::create(path, &Project::new(Settings::default()))?;
        }
        let store = Store::open(path)?;
        let disk = store.read()?;
        let editor = store.load_text(&disk)?;
        let watcher = watch(store.path(), inbox);
        Ok(Self {
            store,
            editor,
            disk,
            history_error: None,
            _watcher: watcher,
        })
    }

    pub fn project(&self) -> &Project {
        self.editor.project()
    }

    /// Re-reads the file under the project lock (so a writer's project file
    /// and history are seen together); returns whether the project changed.
    pub fn reload(&mut self) -> Result<bool> {
        let _lock = self.store.lock()?;
        self.refresh()
    }

    /// Re-reads the file if it changed; returns whether the project changed.
    /// Callers that don't hold the lock should use [`reload`](Self::reload).
    pub fn refresh(&mut self) -> Result<bool> {
        let text = self.store.read()?;
        if text == self.disk {
            return Ok(false);
        }
        // `load_text` takes the history from the sidecar a headless run
        // wrote, or records an "external change" undo step.
        let editor = self.store.load_text(&text)?;
        let changed =
            editor.project() != self.editor.project() || editor.history() != self.editor.history();
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
            match self.store.save(&self.editor) {
                Ok(saved) => {
                    self.disk = saved.text;
                    self.history_error = saved.history_error;
                }
                Err(e) => return (Err(e), true),
            }
        }
        (result, committed || refreshed)
    }
}

/// Watches the project file's directory and reports changes to the file.
fn watch(path: &Path, inbox: Sender<Msg>) -> Option<notify::RecommendedWatcher> {
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
            let _ = inbox.send(Msg::FileChanged);
        }
    })
    .ok()?;
    watcher
        .watch(&dir, notify::RecursiveMode::NonRecursive)
        .ok()?;
    Some(watcher)
}
