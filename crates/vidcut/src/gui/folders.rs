use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

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

#[derive(Clone, Default)]
pub struct Folders(Arc<Mutex<Saved>>);

fn path() -> PathBuf {
    crate::paths::state_dir().join("folders.json")
}

impl Folders {
    pub fn load() -> Self {
        let saved = std::fs::read_to_string(path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Self(Arc::new(Mutex::new(saved)))
    }

    pub fn forget(&self) -> Folders {
        let old = std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()));
        Self(Arc::new(Mutex::new(old)))
    }

    pub fn restore(&self, old: Folders) {
        let old = std::mem::take(&mut *old.0.lock().unwrap_or_else(|e| e.into_inner()));
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = old;
    }

    pub fn get(&self, kind: Kind) -> Option<PathBuf> {
        let mut saved = self.0.lock().unwrap_or_else(|e| e.into_inner());
        saved.slot(kind).clone().filter(|d| d.is_dir())
    }

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
        let written = serde_json::to_string_pretty(&*saved)
            .map_err(std::io::Error::other)
            .and_then(|text| {
                std::fs::create_dir_all(file.parent().expect("in the state dir"))?;
                vidcut_core::temp::write(&file, text.as_bytes())
            });
        if let Err(e) = written {
            eprintln!("vidcut: {}: {e}", file.display());
        }
    }
}
