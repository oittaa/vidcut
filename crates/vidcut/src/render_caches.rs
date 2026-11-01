use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use vidcut_core::preferences::PlaybackFiles;
use vidcut_core::{Error, Result};

fn known_file() -> PathBuf {
    crate::paths::state_dir().join("projects.json")
}

fn stored() -> Vec<PathBuf> {
    std::fs::read_to_string(known_file())
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<PathBuf>>(&t).ok())
        .unwrap_or_default()
}

fn known() -> Vec<PathBuf> {
    stored().into_iter().filter(|p| p.is_file()).collect()
}

pub fn remember(project: &Path) {
    let path = canonical(project);
    let mut all = stored();
    if all.contains(&path) {
        return;
    }
    all.push(path);
    let file = known_file();
    let written = std::fs::create_dir_all(crate::paths::state_dir())
        .and_then(|()| vidcut_core::temp::write(&file, json!(all).to_string().as_bytes()));
    if let Err(e) = written {
        eprintln!("vidcut: {}: {e}", file.display());
    }
}

struct Folder {
    dir: PathBuf,
    projects: Vec<PathBuf>,
    files: usize,
    bytes: u64,
}

fn folder(files: &PlaybackFiles, project: &Path) -> PathBuf {
    files.render_cache_in(project.parent().unwrap_or(Path::new("")))
}

fn canonical(project: &Path) -> PathBuf {
    vidcut_core::dirs::canonical(project).unwrap_or(project.to_path_buf())
}

pub fn sharing(projects: &[PathBuf]) -> Result<impl Fn(&Path) -> bool> {
    let files = crate::preferences::load()?.playback_files;
    let wanted: BTreeSet<PathBuf> = projects
        .iter()
        .map(|p| folder(&files, &canonical(p)))
        .collect();
    Ok(move |project: &Path| wanted.contains(&folder(&files, project)))
}

fn folders(projects: &[PathBuf]) -> Result<Vec<Folder>> {
    let files = crate::preferences::load()?.playback_files;
    let mut by_dir: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for p in projects {
        by_dir.entry(folder(&files, p)).or_default().push(p.clone());
    }
    Ok(by_dir
        .into_iter()
        .map(|(dir, projects)| {
            let (files, bytes) = measure(&dir);
            Folder {
                dir,
                projects,
                files,
                bytes,
            }
        })
        .collect())
}

fn is_video(path: &Path) -> bool {
    path.extension()
        .is_some_and(|x| x == vidcut_core::render_cache::FILE)
}

fn measure(dir: &Path) -> (usize, u64) {
    let (mut files, mut bytes) = (0, 0);
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = e.path();
        if let Some(m) = e.metadata().ok().filter(|m| m.is_file()) {
            files += usize::from(is_video(&path));
            bytes += m.len();
        }
    }
    (files, bytes)
}

pub fn empty(dir: &Path) -> usize {
    let mut n = 0;
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = e.path();
        if path.is_file() && std::fs::remove_file(&path).is_ok() {
            n += usize::from(is_video(&path));
        }
    }
    n
}

fn project_json(path: &Path) -> Value {
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned());
    let location = match crate::system::network(path) {
        true => "network",
        false => "local",
    };
    json!({
        "path": path,
        "name": name,
        "library": path.parent(),
        "location": location,
    })
}

pub fn list() -> Result<Value> {
    let rows: Vec<Value> = folders(&known())?
        .iter()
        .map(|f| {
            json!({
                "folder": f.dir,
                "projects": f.projects.iter().map(|p| project_json(p)).collect::<Vec<_>>(),
                "files": f.files,
                "bytes": f.bytes,
            })
        })
        .collect();
    Ok(json!({ "rows": rows }))
}

pub fn clear(
    projects: &[PathBuf],
    dry_run: bool,
    mut open: impl FnMut(&Path) -> Option<usize>,
) -> Result<Value> {
    if let Some(p) = projects.iter().find(|p| !p.is_file()) {
        return Err(Error::not_found(format!(
            "{}: no such project file",
            p.display()
        )));
    }
    let projects: Vec<PathBuf> = projects.iter().map(|p| canonical(p)).collect();
    let cleared: Vec<Value> = folders(&projects)?
        .into_iter()
        .map(|f| {
            let files = match dry_run {
                true => f.files,
                false => open(&f.dir).unwrap_or_else(|| empty(&f.dir)),
            };
            json!({
                "folder": f.dir,
                "projects": f.projects,
                "files": files,
                "bytes": f.bytes,
            })
        })
        .collect();
    Ok(json!({ "cleared": cleared }))
}
