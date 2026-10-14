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
