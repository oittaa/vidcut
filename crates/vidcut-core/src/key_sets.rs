use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::looks::checked_name;
use crate::shortcuts::{Bindings, Custom};

pub const OWN: &str = "vidcut";

const PRESETS: &[(&str, &str)] = &[
    (
        "Adobe Premiere Pro",
        include_str!("../keys/adobe_premiere_pro.toml"),
    ),
    (
        "Apple Final Cut Pro",
        include_str!("../keys/apple_final_cut_pro.toml"),
    ),
    (
        "Avid Media Composer",
        include_str!("../keys/avid_media_composer.toml"),
    ),
    (
        "Avid Pro Tools",
        include_str!("../keys/avid_pro_tools.toml"),
    ),
];

#[derive(Debug, Clone, PartialEq)]
pub struct KeySet {
    pub name: String,
    pub built_in: bool,
    pub keys: Custom,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Refused {
    pub path: PathBuf,
    pub name: String,
    pub why: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Listing {
    pub sets: Vec<KeySet>,
    pub refused: Vec<Refused>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetFile {
    keys: Custom,
}

pub fn dir() -> PathBuf {
    crate::dirs::config().join("keys")
}

pub fn same(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

pub fn parse(text: &str) -> std::result::Result<Custom, String> {
    let file: SetFile = toml::from_str(text).map_err(|e| e.message().to_string())?;
    Bindings::of(&file.keys).map_err(|e| e.message)?;
    Ok(file.keys)
}

pub fn text(keys: &Custom) -> Result<String> {
    toml::to_string(&SetFile { keys: keys.clone() }).map_err(|e| Error::internal(e.to_string()))
}

pub fn built_in() -> Vec<KeySet> {
    let own = KeySet {
        name: OWN.to_string(),
        built_in: true,
        keys: Custom::new(),
    };
    std::iter::once(own)
        .chain(PRESETS.iter().map(|(name, text)| KeySet {
            name: name.to_string(),
            built_in: true,
            keys: parse(text).expect("a built-in key set is valid"),
        }))
        .collect()
}

fn is_built_in(name: &str) -> bool {
    built_in().iter().any(|s| same(&s.name, name))
}

pub fn read(dir: &Path) -> Listing {
    let mut listing = Listing::default();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return listing;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort_by_key(|p| p.to_string_lossy().to_lowercase());
    for path in files {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let found = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| parse(&text))
            .and_then(|keys| match is_built_in(&name) {
                true => Err(format!("{name} is the name of a built-in key set")),
                false => checked_name("key set", &name)
                    .map(|_| keys)
                    .map_err(|e| e.message),
            });
        match found {
            Ok(keys) => listing.sets.push(KeySet {
                name,
                built_in: false,
                keys,
            }),
            Err(why) => listing.refused.push(Refused { path, name, why }),
        }
    }
    listing
}

pub fn all(dir: &Path) -> Listing {
    let mine = read(dir);
    Listing {
        sets: built_in().into_iter().chain(mine.sets).collect(),
        refused: mine.refused,
    }
}

pub fn find(dir: &Path, name: &str) -> Result<KeySet> {
    let listing = all(dir);
    if let Some(s) = listing.sets.into_iter().find(|s| same(&s.name, name)) {
        return Ok(s);
    }
    if let Some(r) = listing.refused.iter().find(|r| same(&r.name, name)) {
        return Err(Error::invalid(format!("{}: {}", r.path.display(), r.why)));
    }
    Err(Error::not_found(format!(
        "no key set {:?}; `vidcut keys sets` lists them",
        name.trim()
    )))
}

fn path_of(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.toml"))
}

fn mine(dir: &Path, name: &str) -> Option<PathBuf> {
    let listing = read(dir);
    listing
        .sets
        .iter()
        .map(|s| (s.name.clone(), path_of(dir, &s.name)))
        .chain(listing.refused.into_iter().map(|r| (r.name, r.path)))
        .find(|(n, _)| same(n, name))
        .map(|(_, p)| p)
}

pub fn save(
    dir: &Path,
    name: &str,
    keys: &Custom,
    overwrite: bool,
    dry_run: bool,
) -> Result<(KeySet, PathBuf)> {
    let name = checked_name("key set", name)?;
    if is_built_in(&name) {
        return Err(Error::invalid(format!(
            "{name} is a built-in key set and can't be changed; save it under another name"
        )));
    }
    Bindings::of(keys)?;
    let old = mine(dir, &name);
    if old.is_some() && !overwrite {
        return Err(Error::invalid(format!(
            "you have a key set called {name}; pass overwrite to replace it"
        )));
    }
    let path = path_of(dir, &name);
    let set = KeySet {
        name,
        built_in: false,
        keys: keys.clone(),
    };
    if dry_run {
        return Ok((set, path));
    }
    let failed = |e: std::io::Error| Error::usage(format!("{}: {e}", path.display()));
    std::fs::create_dir_all(dir).map_err(failed)?;
    if let Some(old) = old.filter(|o| *o != path) {
        std::fs::rename(&old, &path).map_err(failed)?;
    }
    crate::temp::write(&path, text(keys)?.as_bytes()).map_err(failed)?;
    Ok((set, path))
}

pub fn delete(dir: &Path, name: &str, dry_run: bool) -> Result<PathBuf> {
    if is_built_in(name) {
        return Err(Error::invalid(format!(
            "{} is a built-in key set and can't be deleted",
            name.trim()
        )));
    }
    let Some(path) = mine(dir, name) else {
        return Err(Error::not_found(format!(
            "you have no key set {:?}",
            name.trim()
        )));
    };
    if !dry_run {
        std::fs::remove_file(&path)
            .map_err(|e| Error::usage(format!("{}: {e}", path.display())))?;
    }
    Ok(path)
}

pub fn export(dir: &Path, name: &str, to: &Path, dry_run: bool) -> Result<KeySet> {
    let set = find(dir, name)?;
    if !dry_run {
        crate::temp::write(to, text(&set.keys)?.as_bytes())
            .map_err(|e| Error::usage(format!("{}: {e}", to.display())))?;
    }
    Ok(set)
}

pub fn import(
    dir: &Path,
    from: &Path,
    name: Option<&str>,
    overwrite: bool,
    dry_run: bool,
) -> Result<(KeySet, PathBuf)> {
    let text = std::fs::read_to_string(from)
        .map_err(|e| Error::usage(format!("{}: {e}", from.display())))?;
    let keys = parse(&text).map_err(|why| Error::invalid(format!("{}: {why}", from.display())))?;
    let stem = from
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    save(dir, name.unwrap_or(&stem), &keys, overwrite, dry_run)
}
