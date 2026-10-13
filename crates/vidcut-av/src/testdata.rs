//! Location of test media and generated fixtures, which never live in git.
//!
//! `$VIDCUT_TEST_DATA` overrides the default `~/Downloads/vidcut-tmp`.

use std::path::PathBuf;

/// Root of the test-data directory.
pub fn root() -> PathBuf {
    if let Some(dir) = std::env::var_os("VIDCUT_TEST_DATA") {
        return PathBuf::from(dir);
    }
    vidcut_core::dirs::home()
        .join("Downloads")
        .join("vidcut-tmp")
}

/// Directory for generated fixtures (created on demand).
pub fn fixtures_dir() -> std::io::Result<PathBuf> {
    let dir = root().join("fixtures");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
