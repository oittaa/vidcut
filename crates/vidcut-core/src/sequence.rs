use std::collections::HashSet;
use std::path::Path;

use crate::error::{Error, Result};
use crate::model::ImageSequence;

fn split(name: &str) -> Option<(&str, &str, &str)> {
    let stem_end = name.rfind('.').filter(|&i| i > 0).unwrap_or(name.len());
    let stem = &name[..stem_end];
    let end = stem.rfind(|c: char| c.is_ascii_digit())? + 1;
    let start = stem[..end]
        .char_indices()
        .rev()
        .find(|(_, c)| !c.is_ascii_digit())
        .map_or(0, |(i, c)| i + c.len_utf8());
    Some((&name[..start], &name[start..end], &name[end..]))
}

fn escape(s: &str) -> String {
    s.replace('%', "%%")
}

pub fn find(path: &Path) -> Result<ImageSequence> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let names: HashSet<String> = std::fs::read_dir(dir)
        .map_err(|e| Error::media(format!("{}: {e}", dir.display())))?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    find_among(path, &names)
}

pub fn find_among(path: &Path, names: &HashSet<String>) -> Result<ImageSequence> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| Error::usage(format!("{}: not a file name", path.display())))?;
    let (prefix, digits, suffix) = split(name).ok_or_else(|| {
        Error::usage(format!(
            "{name} has no number: only numbered images make a sequence"
        ))
    })?;
    let n: u64 = digits
        .parse()
        .map_err(|_| Error::usage(format!("{name}: the number {digits} is too big")))?;
    let width = digits.len();
    let padded = width > 1
        && names.iter().any(|other| {
            split(other).is_some_and(|(p, d, s)| {
                p == prefix && s == suffix && d.len() == width && d.starts_with('0')
            })
        });
    let named = |k: u64| match padded {
        true => format!("{prefix}{k:0width$}{suffix}"),
        false => format!("{prefix}{k}{suffix}"),
    };
    if !names.contains(&named(n)) {
        return Err(Error::media(format!("{}: no such file", path.display())));
    }
    let mut first = n;
    while first > 0 && names.contains(&named(first - 1)) {
        first -= 1;
    }
    let mut last = n;
    while names.contains(&named(last + 1)) {
        last += 1;
    }
    let number = if padded {
        format!("%0{width}d")
    } else {
        "%d".to_string()
    };
    Ok(ImageSequence {
        pattern: format!("{}{number}{}", escape(prefix), escape(suffix)),
        first,
        count: last - first + 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_split_around_their_last_number() {
        assert_eq!(
            split("frame_000123.png"),
            Some(("frame_", "000123", ".png"))
        );
        assert_eq!(
            split("shot1_v2.0001.exr"),
            Some(("shot1_v2.", "0001", ".exr"))
        );
        assert_eq!(split("12.tif"), Some(("", "12", ".tif")));
        assert_eq!(split("img7"), Some(("img", "7", "")));
        assert_eq!(split("logo.png"), None);
        assert_eq!(split("image.jp2"), None, "not the extension's digit");
    }

    #[test]
    fn a_non_ascii_character_immediately_before_the_digits() {
        assert_eq!(split("写真0001.exr"), Some(("写真", "0001", ".exr")));
        assert_eq!(split("é0001.png"), Some(("é", "0001", ".png")));
    }

    fn folder(names: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::Builder::new()
            .prefix("vidcut-seq-")
            .tempdir()
            .unwrap();
        for n in names {
            std::fs::write(dir.path().join(n), b"").unwrap();
        }
        dir
    }

    #[test]
    fn padding_shows_in_the_neighbours() {
        let dir = folder(&["c_099999.exr", "c_100000.exr", "c_100001.exr"]);
        let s = find(&dir.path().join("c_100000.exr")).unwrap();
        assert_eq!(
            (s.pattern.as_str(), s.first, s.count),
            ("c_%06d.exr", 99_999, 3)
        );
    }
}
