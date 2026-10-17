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

pub fn run(presets_and_themes: bool, dry_run: bool) -> Result<Value> {
    let open: Vec<PathBuf> = crate::ipc::registry::list()
        .into_iter()
        .filter_map(|i| i.project)
        .filter_map(|p| vidcut_core::store::Store::open(&p).ok())
        .map(|s| s.history_path().to_path_buf())
        .collect();
    let (removed, kept) = plan(&Dirs::here(), presets_and_themes, &open);
    if !dry_run {
        for path in &removed {
            remove(path)?;
        }
    }
    Ok(json!({ "removed": removed, "kept": kept }))
}

pub fn plan(
    dirs: &Dirs,
    presets_and_themes: bool,
    open_histories: &[PathBuf],
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut removed, mut kept) = (Vec::new(), Vec::new());
    let holds_another = |e: &Path| dirs.cache.starts_with(e) || dirs.state.starts_with(e);
    for e in entries(&dirs.config) {
        if holds_another(&e) {
            continue;
        }
        let user = matches!(name(&e), "presets" | "themes");
        match user && !presets_and_themes {
            true => kept.push(e),
            false => removed.push(e),
        }
    }
    removed.extend(
        entries(&dirs.cache)
            .into_iter()
            .filter(|e| !holds_another(e)),
    );
    for e in entries(&dirs.state) {
        if name(&e) != "history" {
            removed.push(e);
            continue;
        }
        for h in entries(&e) {
            match open_histories.contains(&h) {
                true => kept.push(h),
                false => removed.push(h),
            }
        }
    }
    (removed, kept)
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| r.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    out.sort();
    out
}

fn name(path: &Path) -> &str {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
}

fn remove(path: &Path) -> Result<()> {
    let removed = match path.is_dir() {
        true => std::fs::remove_dir_all(path),
        false => std::fs::remove_file(path),
    };
    match removed {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(Error::invalid(format!("removing {}: {e}", path.display())))
        }
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

    #[test]
    fn presets_and_themes_stay_unless_asked_and_open_histories_stay() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        tree(
            r,
            &[
                "config/preferences.toml",
                "config/presets/mine.toml",
                "config/themes/mine.toml",
                "cache/thumbs/a.jpg",
                "state/history/open.jsonl",
                "state/history/closed.jsonl",
                "state/recent.json",
            ],
        );
        let dirs = Dirs {
            config: r.join("config"),
            cache: r.join("cache"),
            state: r.join("state"),
        };
        let (removed, kept) = plan(&dirs, false, &[r.join("state/history/open.jsonl")]);
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
                    "config/themes".into(),
                    "state/history/open.jsonl".into(),
                ],
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
        let dirs = Dirs {
            config: r.join("vidcut"),
            cache: r.join("cache"),
            state: r.join("vidcut/state"),
        };
        let (removed, kept) = plan(&dirs, false, &[r.join("vidcut/state/history/open.jsonl")]);
        assert_eq!(
            (names(r, &removed), names(r, &kept)),
            (
                vec!["vidcut/preferences.toml".to_string()],
                vec!["vidcut/state/history/open.jsonl".to_string()],
            )
        );
    }
}
