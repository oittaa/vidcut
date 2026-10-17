use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::Msg;

const EVERY: Duration = Duration::from_secs(3);

type Files = Vec<(String, PathBuf)>;

pub(super) struct MissingWatch {
    files: Arc<Mutex<(Files, Files)>>,
    wake: Sender<()>,
}

fn gone(path: &Path) -> bool {
    vidcut_core::store::from_another_system(path) || !path.is_file()
}

impl MissingWatch {
    pub(super) fn start(tx: Sender<Msg>) -> Self {
        let files: Arc<Mutex<(Files, Files)>> = Arc::default();
        let (wake, woken) = channel();
        let list = files.clone();
        let _ = crate::threads::spawn(
            "vidcut-missing-media",
            crate::threads::Work::Interactive,
            move || {
                let mut last = (HashSet::new(), HashMap::new());
                loop {
                    let (media, proxies) = list.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    let now = (
                        media
                            .into_iter()
                            .filter(|(_, path)| gone(path))
                            .map(|(id, _)| id)
                            .collect::<HashSet<String>>(),
                        proxies
                            .into_iter()
                            .filter(|(_, path)| gone(path))
                            .collect::<HashMap<String, PathBuf>>(),
                    );
                    if now != last {
                        let (media, proxies) = now.clone();
                        if tx.send(Msg::Missing { media, proxies }).is_err() {
                            return;
                        }
                        last = now;
                    }
                    if let Err(RecvTimeoutError::Disconnected) = woken.recv_timeout(EVERY) {
                        return;
                    }
                }
            },
        );
        Self { files, wake }
    }

    pub(super) fn watch(&self, media: Files, proxies: Files) {
        let mut list = self.files.lock().unwrap_or_else(|e| e.into_inner());
        let files = (media, proxies);
        if *list != files {
            *list = files;
            let _ = self.wake.send(());
        }
    }
}
