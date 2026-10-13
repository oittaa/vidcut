//! Which media files are missing, looked at off the session's thread (a
//! stalled network drive can hold a file check for long), when the project
//! changes and every few seconds, so a drive plugged in or a file moved
//! shows in the Media Pool.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::Msg;

/// How often the files are looked at again.
const EVERY: Duration = Duration::from_secs(3);

/// The watching thread's list of files: media ids and their paths.
pub(super) struct MissingWatch {
    files: Arc<Mutex<Vec<(String, PathBuf)>>>,
    wake: Sender<()>,
}

impl MissingWatch {
    /// Starts the thread; it sends [`Msg::Missing`] when the missing set
    /// changes, and ends with the session.
    pub(super) fn start(tx: Sender<Msg>) -> Self {
        let files: Arc<Mutex<Vec<(String, PathBuf)>>> = Arc::default();
        let (wake, woken) = channel();
        let list = files.clone();
        let _ = crate::threads::spawn(
            "vidcut-missing-media",
            crate::threads::Work::Interactive,
            move || {
                let mut last = HashSet::new();
                loop {
                    let files = list.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    let missing: HashSet<String> = files
                        .into_iter()
                        .filter(|(_, path)| !path.is_file())
                        .map(|(id, _)| id)
                        .collect();
                    if missing != last {
                        if tx.send(Msg::Missing(missing.clone())).is_err() {
                            return;
                        }
                        last = missing;
                    }
                    if let Err(RecvTimeoutError::Disconnected) = woken.recv_timeout(EVERY) {
                        return;
                    }
                }
            },
        );
        Self { files, wake }
    }

    /// The files to watch now; looks at them at once.
    pub(super) fn watch(&self, files: Vec<(String, PathBuf)>) {
        let mut list = self.files.lock().unwrap_or_else(|e| e.into_inner());
        if *list != files {
            *list = files;
            let _ = self.wake.send(());
        }
    }
}
