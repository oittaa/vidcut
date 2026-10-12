//! The folders file dialogs start in, remembered across runs in the state
//! dir (`folders.json`): where media were last imported from, where a
//! project was last opened or created, and where something was last
//! exported (timeline files too).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// What a dialog is for: each remembers its own folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Media,
    Projects,
    Exports,
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    media: Option<PathBuf>,
    #[serde(default)]
    projects: Option<PathBuf>,
    #[serde(default)]
    exports: Option<PathBuf>,
}

impl Saved {
    fn slot(&mut self, kind: Kind) -> &mut Option<PathBuf> {
        match kind {
            Kind::Media => &mut self.media,
            Kind::Projects => &mut self.projects,
            Kind::Exports => &mut self.exports,
        }
    }
}

/// The remembered folders, shared with the dialogs' helper threads.
#[derive(Clone, Default)]
pub struct Folders(Arc<Mutex<Saved>>);

fn path() -> PathBuf {
    crate::paths::state_dir().join("folders.json")
}

impl Folders {
    /// The folders saved last time; none if there is no readable file.
    pub fn load() -> Self {
        let saved = std::fs::read_to_string(path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Self(Arc::new(Mutex::new(saved)))
    }

    /// Where a `kind` dialog starts: the folder remembered for it, while it
    /// is still there.
    pub fn get(&self, kind: Kind) -> Option<PathBuf> {
        let mut saved = self.0.lock().unwrap_or_else(|e| e.into_inner());
        saved.slot(kind).clone().filter(|d| d.is_dir())
    }

    /// Remembers the folder of `picked` (a file, or a folder itself) for
    /// `kind`, and saves. A file that can't be written only loses the
    /// memory for the next run.
    pub fn remember(&self, kind: Kind, picked: &Path) {
        let folder = if picked.is_dir() {
            Some(picked)
        } else {
            picked.parent()
        };
        let Some(folder) = folder.filter(|f| !f.as_os_str().is_empty()) else {
            return;
        };
        let mut saved = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if saved.slot(kind).as_deref() == Some(folder) {
            return;
        }
        *saved.slot(kind) = Some(folder.to_path_buf());
        let file = path();
        let part = file.with_extension("part");
        let written = serde_json::to_string_pretty(&*saved)
            .map_err(std::io::Error::other)
            .and_then(|text| {
                std::fs::create_dir_all(file.parent().expect("in the state dir"))?;
                std::fs::write(&part, text)?;
                std::fs::rename(&part, &file)
            });
        if let Err(e) = written {
            eprintln!("vidcut: {}: {e}", file.display());
        }
    }
}
