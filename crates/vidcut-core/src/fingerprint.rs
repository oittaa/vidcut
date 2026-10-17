use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use sha2::{Digest, Sha256};

pub const WHOLE: u64 = 10 * 1024 * 1024;

const PIECE: u64 = 1024 * 1024;

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

pub fn pictures(first: &Path, middle: &Path, last: &Path) -> std::io::Result<String> {
    let mut hash = Sha256::new();
    for picture in [first, middle, last] {
        hash.update(file(picture)?.as_bytes());
    }
    Ok(hex(&hash.finalize()))
}

pub fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub fn short_digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes)[..16])
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Origin {
    pub name: Arc<str>,
    pub text: Arc<str>,
}

impl Origin {
    pub fn stamp(&self, bytes: &[u8]) -> Vec<u8> {
        let mut file = Vec::with_capacity(4 + self.text.len() + bytes.len());
        file.extend_from_slice(&(self.text.len() as u32).to_le_bytes());
        file.extend_from_slice(self.text.as_bytes());
        file.extend_from_slice(bytes);
        file
    }

    pub fn check<'a>(&self, file: &'a [u8]) -> Option<&'a [u8]> {
        let (len, rest) = file.split_first_chunk::<4>()?;
        let (text, bytes) = rest.split_at_checked(u32::from_le_bytes(*len) as usize)?;
        (text == self.text.as_bytes()).then_some(bytes)
    }
}

pub fn origin(path: &Path, size: u64, mtime_ns: i64) -> Origin {
    type Kept = HashMap<OsString, Vec<(u64, i64, Origin)>>;
    static KEPT: Mutex<Option<Kept>> = Mutex::new(None);
    let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
    let kept = kept.get_or_insert_with(HashMap::new);
    let path = path.as_os_str();
    let known = kept.get(path).and_then(|versions| {
        versions
            .iter()
            .find(|(s, t, _)| (*s, *t) == (size, mtime_ns))
    });
    if let Some((.., o)) = known {
        return o.clone();
    }
    let o = Origin {
        name: format!(
            "{}-{size}-{mtime_ns}",
            short_digest(path.as_encoded_bytes())
        )
        .into(),
        text: format!("{size} {mtime_ns} {}", path.to_string_lossy()).into(),
    };
    kept.entry(path.to_os_string())
        .or_default()
        .push((size, mtime_ns, o.clone()));
    o
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from(DIGITS[usize::from(b >> 4)]));
        s.push(char::from(DIGITS[usize::from(b & 15)]));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_digest_is_the_sha256_of_the_bytes() {
        assert_eq!(
            digest(b"vidcut"),
            "0af8ae7dbfee97fdeea3674c384c91590fa5b19f0156445533f4a166fa5308e0"
        );
    }

    #[test]
    fn a_short_digest_is_the_first_128_bits() {
        assert_eq!(short_digest(b"vidcut"), "0af8ae7dbfee97fdeea3674c384c9159");
    }

    #[test]
    fn an_origin_holds_the_path_size_and_time() {
        let o = origin(Path::new("/media/a.mov"), 1000, 1_700_000_000_000_000_000);
        assert_eq!(
            &*o.name,
            "e0d49d7e2c09594b794f649dea1dcd1e-1000-1700000000000000000"
        );
        assert_eq!(&*o.text, "1000 1700000000000000000 /media/a.mov");
    }

    #[test]
    fn a_stamped_file_gives_its_bytes_back() {
        let o = origin(Path::new("/media/a.mov"), 1000, 1);
        assert_eq!(o.check(&o.stamp(b"pictures")), Some(&b"pictures"[..]));
    }

    #[test]
    fn a_file_made_from_another_origin_gives_nothing() {
        let older = origin(Path::new("/media/a.mov"), 1000, 1);
        let newer = origin(Path::new("/media/a.mov"), 1000, 2);
        assert_eq!(newer.check(&older.stamp(b"pictures")), None);
    }

    #[test]
    fn a_file_without_an_origin_gives_nothing() {
        let o = origin(Path::new("/media/a.mov"), 1000, 1);
        assert_eq!(o.check(b"vct2 pictures"), None);
    }
}
