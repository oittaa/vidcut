use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::model::ImageSequence;

pub const MEDIA_TYPES: &[&str] = &[
    "mp4", "mov", "mkv", "webm", "avi", "mxf", "m4v", "ts", "mts", "m2ts", "mpg", "mpeg", "vob",
    "3gp", "dv", "wmv", "flv", "ogv", "gif", "wav", "aif", "aiff", "flac", "mp3", "m4a", "aac",
    "ogg", "opus", "wma",
];

pub const IMAGE_TYPES: &[&str] = &[
    "png", "jpg", "jpeg", "tif", "tiff", "exr", "dpx", "tga", "bmp", "webp",
];

pub const FRAME_TYPES: &[&str] = &["png", "tif", "tiff", "exr", "dpx", "tga", "bmp"];

pub const SUBTITLE_TYPES: &[&str] = &["srt", "vtt", "webvtt", "ttml", "dfxp", "xml"];

pub const NOT_MEDIA: &str = "not a media file";

pub fn extension(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

pub fn is_media(path: &Path) -> bool {
    let ext = extension(path);
    MEDIA_TYPES.contains(&ext.as_str()) || IMAGE_TYPES.contains(&ext.as_str())
}

pub fn is_image(path: &Path) -> bool {
    IMAGE_TYPES.contains(&extension(path).as_str())
}

pub fn sort_by_name(paths: &mut [PathBuf]) {
    paths.sort_by_cached_key(|p| {
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        (name.to_lowercase(), p.clone())
    });
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Folder {
    pub path: PathBuf,
    pub files: Vec<PathBuf>,
    pub folders: Vec<Folder>,
    pub error: Option<String>,
}

impl Folder {
    pub fn name(&self) -> String {
        match self.path.file_name() {
            Some(name) => name.to_string_lossy().into_owned(),
            None => self
                .path
                .to_string_lossy()
                .trim_end_matches(['/', '\\'])
                .to_string(),
        }
    }

    pub fn runs_here(&self) -> Vec<(PathBuf, ImageSequence)> {
        let names: HashSet<String> = self
            .files
            .iter()
            .filter_map(|f| Some(f.file_name()?.to_str()?.to_string()))
            .collect();
        let mut taken: HashSet<PathBuf> = HashSet::new();
        let mut runs = Vec::new();
        for file in &self.files {
            if taken.contains(file) || !is_image(file) {
                continue;
            }
            let Some(s) = crate::sequence::find_among(file, &names)
                .ok()
                .filter(|s| s.count >= 2)
            else {
                continue;
            };
            taken.extend((0..s.count).map(|k| s.file(&self.path, k)));
            runs.push((file.clone(), s));
        }
        runs
    }

    pub fn runs(&self) -> Vec<(PathBuf, ImageSequence)> {
        let mut runs = self.runs_here();
        for f in &self.folders {
            runs.extend(f.runs());
        }
        runs
    }
}

pub fn read(dir: &Path) -> Result<Folder> {
    let path =
        crate::dirs::canonical(dir).map_err(|e| Error::media(format!("{}: {e}", dir.display())))?;
    if !path.is_dir() {
        return Err(Error::usage(format!(
            "{} is not a folder; add files with media.add",
            dir.display()
        )));
    }
    let folder = read_in(path, &mut Vec::new());
    match &folder.error {
        Some(e) => Err(Error::media(format!("{}: {e}", dir.display()))),
        None => Ok(folder),
    }
}

pub const LOOPS: &str = "the same folder as one above it";

#[cfg(unix)]
type DirId = (u64, u64);

#[cfg(not(unix))]
type DirId = PathBuf;

#[cfg(unix)]
fn dir_id(path: &Path) -> Option<DirId> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

#[cfg(not(unix))]
fn dir_id(path: &Path) -> Option<DirId> {
    crate::dirs::canonical(path).ok()
}

fn read_in(path: PathBuf, above: &mut Vec<DirId>) -> Folder {
    let id = dir_id(&path);
    if id.as_ref().is_some_and(|id| above.contains(id)) {
        return Folder {
            path,
            error: Some(LOOPS.into()),
            ..Folder::default()
        };
    }
    let entries = match std::fs::read_dir(&path) {
        Ok(entries) => entries,
        Err(e) => {
            return Folder {
                path,
                error: Some(e.to_string()),
                ..Folder::default()
            };
        }
    };
    let (mut files, mut dirs) = (Vec::new(), Vec::new());
    for e in entries.flatten() {
        if hidden(&e) {
            continue;
        }
        let Ok(kind) = e.file_type() else {
            continue;
        };
        if kind.is_dir() {
            dirs.push(e.path());
        } else if e.path().is_file() {
            files.push(e.path());
        }
    }
    sort_by_name(&mut files);
    sort_by_name(&mut dirs);
    let held = id.is_some();
    above.extend(id);
    let folders = dirs.into_iter().map(|d| read_in(d, above)).collect();
    if held {
        above.pop();
    }
    Folder {
        path,
        files,
        folders,
        error: None,
    }
}

pub(crate) fn hidden(e: &std::fs::DirEntry) -> bool {
    e.file_name().to_string_lossy().starts_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_is_known_by_its_extension_in_any_case() {
        assert!(is_media(Path::new("/shoot/A001.MOV")));
        assert!(is_media(Path::new("/shoot/frame_0001.exr")));
        assert!(!is_media(Path::new("/shoot/notes.txt")));
        assert!(!is_media(Path::new("/shoot/README")));
    }

    #[test]
    fn names_sort_ignoring_case() {
        let mut paths: Vec<PathBuf> = ["/s/B-close.mp4", "/s/a-wide.mp4", "/s/clip.mp4"]
            .iter()
            .map(PathBuf::from)
            .collect();
        sort_by_name(&mut paths);
        assert_eq!(
            paths,
            ["/s/a-wide.mp4", "/s/B-close.mp4", "/s/clip.mp4"].map(PathBuf::from)
        );
    }

    #[test]
    fn a_folder_is_named_by_its_last_part() {
        let folder = Folder {
            path: PathBuf::from("/footage/Day 1"),
            ..Folder::default()
        };
        assert_eq!(folder.name(), "Day 1");
    }

    #[cfg(windows)]
    #[test]
    fn a_drive_is_named_by_its_letter() {
        let drive = Folder {
            path: PathBuf::from(r"E:\"),
            ..Folder::default()
        };
        assert_eq!(drive.name(), "E:");
    }

    #[test]
    fn numbered_images_two_or_more_in_a_row_make_runs() {
        let shoot = Folder {
            path: PathBuf::from("/shoot"),
            files: [
                "img_0001.png",
                "img_0002.png",
                "img_0003.png",
                "logo_1.png",
                "take_2.mp4",
            ]
            .iter()
            .map(|n| Path::new("/shoot").join(n))
            .collect(),
            ..Folder::default()
        };
        let runs: Vec<(String, u64)> = shoot
            .runs()
            .into_iter()
            .map(|(_, s)| (s.display(), s.count))
            .collect();
        assert_eq!(runs, [("img_[0001-0003].png".to_string(), 3)]);
    }

    #[test]
    fn hidden_files_and_folders_are_left_out() {
        let dir = tempfile::Builder::new()
            .prefix("vidcut-folder-")
            .tempdir()
            .unwrap();
        for name in ["b.mp4", "a.mp4", ".DS_Store", ".cache/x.mp4", "Day 1/c.mp4"] {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"").unwrap();
        }
        let folder = read(dir.path()).unwrap();
        let names = |f: &Folder| -> Vec<String> {
            f.files
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(names(&folder), ["a.mp4", "b.mp4"]);
        assert_eq!(folder.folders.len(), 1);
        assert_eq!(folder.folders[0].name(), "Day 1");
        assert_eq!(names(&folder.folders[0]), ["c.mp4"]);
    }
}
