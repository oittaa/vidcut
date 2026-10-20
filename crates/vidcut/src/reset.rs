use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use vidcut_core::{Error, Result};

pub struct Dirs {
    pub config: PathBuf,
    pub cache: PathBuf,
    pub state: PathBuf,
}

impl Dirs {
    fn here() -> Self {
        Self {
            config: crate::paths::config_dir(),
            cache: crate::paths::cache_dir(),
            state: crate::paths::state_dir(),
        }
    }
}

pub fn run(saved: bool, dry_run: bool) -> Result<Value> {
    let running = crate::ipc::registry::list();
    if let Some(other) = running.iter().find(|i| i.pid != std::process::id()) {
        return Err(Error::conflict(format!(
            "a vidcut GUI is running (pid {}), and it would write its settings back: close it \
             first, or use Reset vidcut… in its Preferences",
            other.pid
        )));
    }
    let dirs = Dirs::here();
    let mut keep: Vec<PathBuf> = running
        .into_iter()
        .filter_map(|i| i.project)
        .filter_map(|p| vidcut_core::store::Store::open(&p).ok())
        .map(|s| s.history_path().to_path_buf())
        .collect();
    if !saved {
        keep.extend(["presets", "themes", "looks"].map(|d| dirs.config.join(d)));
    }
    if let Ok(p) = crate::preferences::load()
        && p.proxies.folder.is_absolute()
    {
        keep.push(p.proxies.folder);
    }
    let (dirs, keep) = resolved(dirs, keep);
    let (removed, kept) = plan(&dirs, &keep)?;
    if !dry_run {
        for path in &removed {
            remove(path)?;
        }
    }
    Ok(json!({ "removed": removed, "kept": kept }))
}

fn resolved(dirs: Dirs, keep: Vec<PathBuf>) -> (Dirs, Vec<PathBuf>) {
    let real = |p: PathBuf| vidcut_core::dirs::canonical(&p).unwrap_or(p);
    let dirs = Dirs {
        config: real(dirs.config),
        cache: real(dirs.cache),
        state: real(dirs.state),
    };
    (dirs, keep.into_iter().map(real).collect())
}

pub fn plan(dirs: &Dirs, keep: &[PathBuf]) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let roots = [&dirs.config, &dirs.cache, &dirs.state];
    let mut found = (Vec::new(), Vec::new());
    for root in roots {
        let others: Vec<&Path> = roots
            .iter()
            .filter(|r| **r != root)
            .map(|r| r.as_path())
            .collect();
        sweep(root, keep, &others, &mut found)?;
    }
    Ok(found)
}

fn sweep(
    dir: &Path,
    keep: &[PathBuf],
    others: &[&Path],
    found: &mut (Vec<PathBuf>, Vec<PathBuf>),
) -> Result<()> {
    for e in entries(dir)? {
        if others.iter().any(|o| o.starts_with(&e)) {
            continue;
        }
        if keep.contains(&e) {
            found.1.push(e);
        } else if keep.iter().any(|k| k.starts_with(&e)) {
            sweep(&e, keep, others, found)?;
        } else {
            found.0.push(e);
        }
    }
    Ok(())
}

fn entries(dir: &Path) -> Result<Vec<PathBuf>> {
    let unreadable =
        |e: std::io::Error| Error::conflict(format!("can't read {}: {e}", dir.display()));
    let listing = match std::fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(unreadable(e)),
    };
    let mut out = listing
        .map(|e| e.map(|e| e.path()).map_err(unreadable))
        .collect::<Result<Vec<_>>>()?;
    out.sort();
    Ok(out)
}

fn remove(path: &Path) -> Result<()> {
    let removed = match path.is_dir() {
        true => std::fs::remove_dir_all(path),
        false => std::fs::remove_file(path),
    };
    match removed {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(Error::conflict(format!(
            "couldn't remove {}: {e}",
            path.display()
        ))),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(root: &Path, files: &[&str]) {
        for f in files {
            let path = root.join(f);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "x").unwrap();
        }
    }

    fn names(root: &Path, paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    fn dirs(r: &Path, config: &str, cache: &str, state: &str) -> Dirs {
        Dirs {
            config: r.join(config),
            cache: r.join(cache),
            state: r.join(state),
        }
    }

    #[test]
    fn kept_folders_and_an_open_history_stay_and_the_rest_goes() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        tree(
            r,
            &[
                "config/preferences.toml",
                "config/presets/mine.toml",
                "cache/thumbs/a.jpg",
                "state/history/open.jsonl",
                "state/history/closed.jsonl",
                "state/recent.json",
            ],
        );
        let keep = [r.join("config/presets"), r.join("state/history/open.jsonl")];
        let (removed, kept) = plan(&dirs(r, "config", "cache", "state"), &keep).unwrap();
        assert_eq!(
            (names(r, &removed), names(r, &kept)),
            (
                vec![
                    "config/preferences.toml".to_string(),
                    "cache/thumbs".into(),
                    "state/history/closed.jsonl".into(),
                    "state/recent.json".into(),
                ],
                vec![
                    "config/presets".to_string(),
                    "state/history/open.jsonl".into()
                ],
            )
        );
    }

    #[test]
    fn a_kept_path_written_with_dot_dot_still_stays() {
        let root = tempfile::tempdir().unwrap();
        let r = &vidcut_core::dirs::canonical(root.path()).unwrap();
        tree(r, &["cache/thumbs/a.jpg", "cache/shared/a.mkv"]);
        let (d, keep) = resolved(
            dirs(r, "config", "cache", "state"),
            vec![r.join("cache/thumbs/../shared")],
        );
        let (removed, kept) = plan(&d, &keep).unwrap();
        assert_eq!(
            (names(r, &removed), names(r, &kept)),
            (
                vec!["cache/thumbs".to_string()],
                vec!["cache/shared".to_string()]
            )
        );
    }

    #[test]
    fn a_config_dir_that_cant_be_read_fails_the_reset() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        tree(r, &["config"]);
        assert!(plan(&dirs(r, "config", "cache", "state"), &[]).is_err());
    }

    #[test]
    fn a_proxy_folder_set_inside_the_cache_stays() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        tree(r, &["cache/thumbs/a.jpg", "cache/shared/a.mkv"]);
        let (removed, kept) = plan(
            &dirs(r, "config", "cache", "state"),
            &[r.join("cache/shared")],
        )
        .unwrap();
        assert_eq!(
            (names(r, &removed), names(r, &kept)),
            (
                vec!["cache/thumbs".to_string()],
                vec!["cache/shared".to_string()]
            )
        );
    }

    #[test]
    fn a_state_dir_inside_the_config_dir_keeps_its_open_histories() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        tree(
            r,
            &["vidcut/preferences.toml", "vidcut/state/history/open.jsonl"],
        );
        let keep = [r.join("vidcut/state/history/open.jsonl")];
        let (removed, kept) = plan(&dirs(r, "vidcut", "cache", "vidcut/state"), &keep).unwrap();
        assert_eq!(
            (names(r, &removed), names(r, &kept)),
            (
                vec!["vidcut/preferences.toml".to_string()],
                vec!["vidcut/state/history/open.jsonl".to_string()],
            )
        );
    }
}
