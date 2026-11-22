use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LutList {
    pub folder: PathBuf,
    pub folders: Vec<PathBuf>,
    pub luts: Vec<Lut>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lut {
    pub name: String,
    pub menu: Vec<String>,
    pub path: PathBuf,
}

pub fn dir() -> PathBuf {
    crate::dirs::config().join("LUT")
}

impl LutList {
    pub fn read(folders: &[PathBuf]) -> Self {
        let folder = dir();
        let mut found = Vec::new();
        let mut seen = HashSet::new();
        for root in std::iter::once(&folder).chain(folders) {
            walk(root, &mut Vec::new(), &mut seen, &mut found);
        }
        found.sort_by(|a: &(String, Lut), b| a.0.cmp(&b.0));
        Self {
            folder,
            folders: folders.to_vec(),
            luts: found.into_iter().map(|(_, lut)| lut).collect(),
        }
    }

    pub fn find(&self, path: &Path) -> Option<&Lut> {
        self.luts.iter().find(|l| l.path == path)
    }
}

fn walk(
    dir: &Path,
    menu: &mut Vec<String>,
    seen: &mut HashSet<PathBuf>,
    found: &mut Vec<(String, Lut)>,
) {
    let Ok(real) = crate::dirs::canonical(dir) else {
        return;
    };
    if !seen.insert(real) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let file = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            menu.push(file);
            walk(&path, menu, seen, found);
            menu.pop();
        } else if is_cube(&path) && path.is_file() {
            let key = menu
                .iter()
                .map(String::as_str)
                .chain([file.as_str()])
                .collect::<Vec<_>>()
                .join("/");
            let name = path
                .file_stem()
                .map_or(String::new(), |s| s.to_string_lossy().into_owned());
            let menu = menu.clone();
            found.push((key, Lut { name, menu, path }));
        }
    }
}

fn is_cube(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cube"))
}
