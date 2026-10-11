//! Numbered image sequences (p. 366): the run of files a numbered image
//! belongs to, as one clip.

use std::collections::HashSet;
use std::path::Path;

use crate::error::{Error, Result};
use crate::model::ImageSequence;

/// A file name split around its number: the last run of digits before the
/// extension (`shot1_v2.0001.exr`: `shot1_v2.`, `0001`, `.exr`).
fn split(name: &str) -> Option<(&str, &str, &str)> {
    let stem_end = name.rfind('.').filter(|&i| i > 0).unwrap_or(name.len());
    let stem = &name[..stem_end];
    let end = stem.rfind(|c: char| c.is_ascii_digit())? + 1;
    // The index is the character's first byte. Step over the whole character:
    // the one before the digits may not be ASCII (`写真0001.exr`).
    let start = stem[..end]
        .char_indices()
        .rev()
        .find(|(_, c)| !c.is_ascii_digit())
        .map_or(0, |(i, c)| i + c.len_utf8());
    Some((&name[..start], &name[start..end], &name[end..]))
}

/// `%` as FFmpeg's image2 patterns write it.
fn escape(s: &str) -> String {
    s.replace('%', "%%")
}

/// The numbered run `path` belongs to: the files in its folder named like it
/// with other numbers, with no number missing in between. Numbers are
/// zero-padded to a width when the name or its neighbours show leading
/// zeros (`frame_000123.png`), else unpadded (`img_9.png`, `img_10.png`).
pub fn find(path: &Path) -> Result<ImageSequence> {
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
    let dir = path.parent().unwrap_or(Path::new("."));
    let names: HashSet<String> = std::fs::read_dir(dir)
        .map_err(|e| Error::media(format!("{}: {e}", dir.display())))?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    // Padded to the number's width if it, or another file of the same
    // width, starts with a zero.
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
        // The byte after that character's start is still inside it. Stepping
        // one byte used to slice there and panic; the number is still found.
        assert_eq!(split("写真0001.exr"), Some(("写真", "0001", ".exr")));
        assert_eq!(split("é0001.png"), Some(("é", "0001", ".png")));
    }

    fn folder(names: &[&str]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "vidcut-seq-{}-{}",
            std::process::id(),
            names.join("+").len()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for n in names {
            std::fs::write(dir.join(n), b"").unwrap();
        }
        dir
    }

    #[test]
    fn a_run_is_the_numbers_in_a_row_around_the_file() {
        let dir = folder(&[
            "f_0008.png",
            "f_0009.png",
            "f_0010.png",
            "f_0011.png",
            "f_0013.png",
            "f_0010.jpg",
            "g_0010.png",
        ]);
        let s = find(&dir.join("f_0010.png")).unwrap();
        assert_eq!((s.pattern.as_str(), s.first, s.count), ("f_%04d.png", 8, 4));
        assert_eq!(s.name(11), "f_0011.png");
        assert_eq!(s.display(), "f_[0008-0011].png");
        // Past the gap, a run of its own.
        let s = find(&dir.join("f_0013.png")).unwrap();
        assert_eq!((s.first, s.count), (13, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unpadded_numbers_and_percent_signs() {
        let dir = folder(&["50% 9.png", "50% 10.png", "50% 11.png"]);
        let s = find(&dir.join("50% 10.png")).unwrap();
        assert_eq!(
            (s.pattern.as_str(), s.first, s.count),
            ("50%% %d.png", 9, 3)
        );
        assert_eq!(s.name(10), "50% 10.png");
        assert_eq!(s.display(), "50% [9-11].png");
        assert!(find(&dir.join("50% 12.png")).is_err(), "no such file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn padding_shows_in_the_neighbours() {
        // 123456 has no leading zero, but its folder's other files do.
        let dir = folder(&["c_099999.exr", "c_100000.exr", "c_100001.exr"]);
        let s = find(&dir.join("c_100000.exr")).unwrap();
        assert_eq!(
            (s.pattern.as_str(), s.first, s.count),
            ("c_%06d.exr", 99_999, 3)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
