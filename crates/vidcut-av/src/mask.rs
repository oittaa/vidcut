mod kernels;
mod net;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use vidcut_core::catalog::{self, File};

use crate::error::{Error, Result};
use crate::models::{self, Fetch, Need};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Tiny,
    BasePlus,
    Large,
}

struct Config {
    embed: u32,
    heads: u32,
    stages: [u32; 4],
    global: &'static [u32],
    windows: [u32; 4],
    bkg: u32,
}

impl Size {
    fn config(self) -> Config {
        match self {
            Self::Tiny => Config {
                embed: 96,
                heads: 1,
                stages: [1, 2, 7, 2],
                global: &[5, 7, 9],
                windows: [8, 4, 14, 7],
                bkg: 7,
            },
            Self::BasePlus => Config {
                embed: 112,
                heads: 2,
                stages: [2, 3, 16, 3],
                global: &[12, 16, 20],
                windows: [8, 4, 14, 7],
                bkg: 14,
            },
            Self::Large => Config {
                embed: 144,
                heads: 2,
                stages: [2, 6, 36, 4],
                global: &[23, 33, 43],
                windows: [8, 4, 16, 8],
                bkg: 7,
            },
        }
    }

    fn model(self) -> &'static catalog::Model {
        match self {
            Self::Tiny => &catalog::SAM21_TINY,
            Self::BasePlus => &catalog::SAM21_BASE_PLUS,
            Self::Large => &catalog::SAM21_LARGE,
        }
    }

    fn file(self) -> &'static File {
        &self.model().files[0]
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Click {
    pub at: [f64; 2],
    pub positive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub logits: Vec<f32>,
    pub object: f32,
}

pub fn needed(dir: &Path, size: Size) -> Vec<Need> {
    let f = size.file();
    match models::present(dir, f) {
        true => Vec::new(),
        false => vec![Need {
            title: f.title.into(),
            bytes: Some(f.bytes),
            system: false,
        }],
    }
}

pub fn get_ready(dir: &Path, size: Size, download: bool) -> Result<()> {
    let missing = needed(dir, size);
    if missing.is_empty() {
        return Ok(());
    }
    if !download {
        return Err(Error::Missing(format!(
            "{} isn't on this computer yet: pass download (--download) to download it into {}, \
             checked against its SHA-256",
            models::listed(&missing),
            dir.display()
        )));
    }
    models::network();
    models::fetch(&Fetch::of(size.file()), dir)?;
    Ok(())
}

static NET: Mutex<Option<(Size, net::Net)>> = Mutex::new(None);

pub fn find(
    dir: &Path,
    size: Size,
    (w, h, pixels): (u32, u32, &[[f32; 3]]),
    subjects: &[Vec<Click>],
) -> Result<Vec<Found>> {
    if w == 0 || h == 0 || pixels.len() != w as usize * h as usize {
        return Err(Error::Invalid(format!(
            "a subject mask needs {w}x{h} pixels, not {}",
            pixels.len()
        )));
    }
    get_ready(dir, size, false)?;
    let mut held = NET.lock().unwrap_or_else(PoisonError::into_inner);
    if held.as_ref().is_none_or(|(s, _)| *s != size) {
        *held = None;
        let path = dir.join(size.file().name);
        *held = Some((
            size,
            net::Net::load(&path, size.model().title, &size.config())?,
        ));
    }
    let Some((_, net)) = held.as_ref() else {
        return Err(Error::Invalid("no subject mask model".into()));
    };
    let found: Result<Vec<Found>> = (|| {
        net.encode((w, h), pixels)?;
        if let Some(dir) = dumped()? {
            net.dump(&dir)?;
        }
        subjects.iter().map(|c| net.decode(c)).collect()
    })();
    if found.is_err() {
        *held = None;
    }
    found
}

fn dumped() -> Result<Option<PathBuf>> {
    let Some(dir) = std::env::var_os("VIDCUT_MASK_DUMP") else {
        return Ok(None);
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    Ok(Some(dir))
}

fn dump_index() -> usize {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
