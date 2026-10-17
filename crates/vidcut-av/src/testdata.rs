use std::path::PathBuf;

pub fn root() -> PathBuf {
    if let Some(dir) = std::env::var_os("VIDCUT_TEST_DATA") {
        return PathBuf::from(dir);
    }
    vidcut_core::dirs::home()
        .join("Downloads")
        .join("vidcut-tmp")
}

pub fn fixtures_dir() -> std::io::Result<PathBuf> {
    let dir = root().join("fixtures");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn temp_dir() -> PathBuf {
    let temp = std::env::temp_dir();
    vidcut_core::dirs::canonical(&temp).unwrap_or(temp)
}
