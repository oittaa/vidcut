//! Media fingerprints: a SHA-256 that tells a moved file from another one
//! with the same name, size and streams, without reading all of a big file.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use sha2::{Digest, Sha256};

/// Files up to this size are hashed whole: their fingerprint is their
/// `sha256sum`.
pub const WHOLE: u64 = 10 * 1024 * 1024;

/// Bigger files are hashed by three pieces this long: from the start, the
/// middle and the end.
const PIECE: u64 = 1024 * 1024;

/// The fingerprint of the file at `path`, in lowercase hex.
pub fn file(path: &Path) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    let size = f.metadata()?.len();
    let mut hash = Sha256::new();
    if size <= WHOLE {
        let mut buf = Vec::with_capacity(size as usize);
        f.read_to_end(&mut buf)?;
        hash.update(&buf);
    } else {
        let mut buf = vec![0; PIECE as usize];
        for at in [0, size / 2 - PIECE / 2, size - PIECE] {
            f.seek(SeekFrom::Start(at))?;
            f.read_exact(&mut buf)?;
            hash.update(&buf);
        }
    }
    Ok(hex(&hash.finalize()))
}

/// An image sequence's fingerprint: of its first, middle and last
/// pictures' (a render's first picture is often plain black).
pub fn pictures(first: &Path, middle: &Path, last: &Path) -> std::io::Result<String> {
    let mut hash = Sha256::new();
    for picture in [first, middle, last] {
        hash.update(file(picture)?.as_bytes());
    }
    Ok(hex(&hash.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_files_whole_big_ones_by_three_pieces() {
        let dir = tempfile::tempdir().unwrap();
        // A small file's fingerprint is its sha256sum.
        let small = dir.path().join("small");
        std::fs::write(&small, b"abc").unwrap();
        assert_eq!(
            file(&small).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // A big one changes with its pieces, not between them.
        let big = dir.path().join("big");
        let size = WHOLE as usize + 3 * PIECE as usize;
        let mut data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        std::fs::write(&big, &data).unwrap();
        let before = file(&big).unwrap();
        data[PIECE as usize + 10] ^= 1;
        std::fs::write(&big, &data).unwrap();
        assert_eq!(file(&big).unwrap(), before, "between the pieces");
        for at in [0, size / 2, size - 1] {
            let mut changed = data.clone();
            changed[at] ^= 1;
            std::fs::write(&big, &changed).unwrap();
            assert_ne!(file(&big).unwrap(), before, "byte {at}");
        }
    }

    #[test]
    fn sequences_by_their_first_middle_and_last_pictures() {
        let dir = tempfile::tempdir().unwrap();
        let p = |n: u8| {
            let path = dir.path().join(format!("f{n}.png"));
            std::fs::write(&path, [n]).unwrap();
            path
        };
        let (a, b, c, d) = (p(1), p(2), p(3), p(4));
        let fp = pictures(&a, &b, &c).unwrap();
        assert_ne!(fp, pictures(&a, &b, &d).unwrap());
        assert_ne!(fp, pictures(&a, &d, &c).unwrap());
        assert_eq!(fp, pictures(&a, &b, &c).unwrap());
    }
}
