use std::ffi::{CString, c_int, c_void};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::export::{CANCELLED, Progress, Stage};
use crate::ffmpeg::ffi;

pub use vidcut_core::catalog::File;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Need {
    pub title: String,
    pub bytes: Option<u64>,
    pub system: bool,
}

pub fn listed(needs: &[Need]) -> String {
    let names: Vec<String> = needs
        .iter()
        .map(|n| match n.bytes {
            Some(bytes) => format!("{} ({})", n.title, size(bytes)),
            None => format!("{} (size unknown)", n.title),
        })
        .collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

pub(crate) fn network() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        crate::init();
        // SAFETY: FFmpeg's network setup, called once before any network protocol is used.
        unsafe { ffi::avformat_network_init() };
    });
}

pub fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1_000_000_000 => format!("{:.1} GB", b as f64 / 1e9),
        b if b >= 1_000_000 => format!("{:.0} MB", b as f64 / 1e6),
        b => format!("{:.0} kB", b as f64 / 1e3),
    }
}

pub fn present(dir: &Path, f: &File) -> bool {
    f.present(dir)
}

pub fn sha256_of(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

pub fn verify(dir: &Path, files: &[File]) -> Result<()> {
    for f in files {
        let have = sha256_of(&dir.join(f.name))?;
        if have != f.sha256 {
            return Err(Error::Invalid(format!(
                "{}: its SHA-256 is {have}, the catalog has {}",
                dir.join(f.name).display(),
                f.sha256
            )));
        }
    }
    Ok(())
}

pub struct Inspected {
    pub found: vidcut_core::catalog::Found,
    pub loaded: Option<&'static str>,
}

pub fn inspect(
    model: &'static vidcut_core::catalog::Model,
    dir: &Path,
    download: bool,
) -> Result<Inspected> {
    let found = vidcut_core::catalog::Found::default();
    match model.id {
        "parakeet-ultra" | "qwen3-asr" | "whisper-large-v3" | "silero-vad" | "sortformer"
        | "moss" => {
            let file = model
                .files
                .first()
                .ok_or_else(|| Error::Invalid(format!("{} has no file", model.title)))?;
            Ok(Inspected {
                found: crate::transcribe::inspect(dir, file, download)?,
                loaded: Some("transcribe.cpp"),
            })
        }
        "dpdfnet" => {
            crate::isolate::inspect(dir, download)?;
            Ok(Inspected {
                found,
                loaded: Some("tract"),
            })
        }
        "nanovsr" => {
            crate::super_scale::nanovsr::inspect(dir, download)?;
            Ok(Inspected {
                found,
                loaded: Some("vidcut"),
            })
        }
        "flashvsr" => {
            crate::super_scale::flashvsr::inspect(dir, download)?;
            Ok(Inspected {
                found,
                loaded: Some("vidcut, on the GPU"),
            })
        }
        id if crate::interpolate::of(id).is_some() => {
            crate::interpolate::inspect(
                dir,
                crate::interpolate::of(id).expect("listed"),
                download,
            )?;
            Ok(Inspected {
                found,
                loaded: Some("vidcut, on the GPU"),
            })
        }
        _ => Err(Error::Invalid(format!(
            "vidcut has no check for {}",
            model.title
        ))),
    }
}

unsafe extern "C" fn interrupted(opaque: *mut c_void) -> c_int {
    // SAFETY: opaque is the AtomicBool that download() keeps alive while the connection is open.
    let flag = unsafe { &*(opaque as *const AtomicBool) };
    c_int::from(flag.load(Ordering::Relaxed))
}

struct Connection(*mut ffi::AVIOContext);

impl Drop for Connection {
    fn drop(&mut self) {
        // SAFETY: a context avio_open2 made, closed once.
        unsafe { ffi::avio_closep(&mut self.0) };
    }
}

fn ffmpeg_error(context: String, code: c_int) -> Error {
    Error::Ffmpeg {
        context,
        source: crate::ffmpeg::Error::from(code),
    }
}

fn open(url: &str, stop: &Arc<AtomicBool>) -> Result<Connection> {
    let c_url = CString::new(url).map_err(|e| Error::Invalid(e.to_string()))?;
    let callback = ffi::AVIOInterruptCB {
        callback: Some(interrupted),
        opaque: Arc::as_ptr(stop) as *mut c_void,
    };
    let mut options: *mut ffi::AVDictionary = std::ptr::null_mut();
    let mut ctx: *mut ffi::AVIOContext = std::ptr::null_mut();
    // SAFETY: FFmpeg's documented calls; every pointer outlives them, and the dictionary is freed here.
    let opened = unsafe {
        ffi::av_dict_set(
            &mut options,
            c"rw_timeout".as_ptr(),
            c"30000000".as_ptr(),
            0,
        );
        let r = ffi::avio_open2(
            &mut ctx,
            c_url.as_ptr(),
            ffi::AVIO_FLAG_READ,
            &callback,
            &mut options,
        );
        ffi::av_dict_free(&mut options);
        r
    };
    if opened == ffi::AVERROR_PROTOCOL_NOT_FOUND {
        return Err(Error::Missing(format!(
            "this FFmpeg can't download over HTTPS: download {url} yourself"
        )));
    }
    if opened < 0 {
        return Err(ffmpeg_error(format!("downloading {url}"), opened));
    }
    Ok(Connection(ctx))
}

pub(crate) struct Fetch {
    pub(crate) url: String,
    pub(crate) name: String,
    pub(crate) title: String,
    pub(crate) bytes: u64,
    pub(crate) sha256: &'static str,
}

impl Fetch {
    pub(crate) fn of(f: &File) -> Self {
        Self {
            url: f.url.to_string(),
            name: f.name.to_string(),
            title: f.title.to_string(),
            bytes: f.bytes,
            sha256: f.sha256,
        }
    }
}

pub(crate) fn fetch(f: &Fetch, dir: &Path) -> Result<PathBuf> {
    let target = dir.join(&f.name);
    std::fs::create_dir_all(target.parent().unwrap_or(dir))?;
    let had = target.is_file();
    let _held = vidcut_core::temp::lock(&target)?;
    if !had && target.is_file() {
        return Ok(target);
    }
    crate::progress::note(crate::export::Note::Downloading {
        name: format!("{} ({})", f.title, size(f.bytes)),
    });
    let part = dir.join(format!("{}.part", f.name));
    let stop = crate::cancel::current().unwrap_or_default();
    let started = Instant::now();
    let url = f.url.clone();
    let mut hash = Sha256::new();
    let mut have = 0u64;
    if let Ok(mut old) = std::fs::File::open(&part)
        && old.metadata().is_ok_and(|m| m.len() < f.bytes)
    {
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = old.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
            have += n as u64;
        }
    }
    let connection = open(&url, &stop)?;
    if have > 0 {
        // SAFETY: an open context; FFmpeg's HTTP protocol asks the server for the rest.
        let at = unsafe { ffi::avio_seek(connection.0, have as i64, libc::SEEK_SET) };
        if at != have as i64 {
            (hash, have) = (Sha256::new(), 0);
            drop(connection);
            download_from(f, &target, &part, open(&url, &stop)?, hash, have, started)?;
            return Ok(target);
        }
    }
    download_from(f, &target, &part, connection, hash, have, started)?;
    Ok(target)
}

fn download_from(
    f: &Fetch,
    target: &Path,
    part: &Path,
    connection: Connection,
    mut hash: Sha256,
    mut have: u64,
    started: Instant,
) -> Result<()> {
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(have > 0)
        .write(true)
        .truncate(have == 0)
        .open(part)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if crate::cancel::cancelled() {
            return Err(Error::Invalid(CANCELLED.into()));
        }
        // SAFETY: an open context and a buffer of the length given.
        let n = unsafe { ffi::avio_read(connection.0, buf.as_mut_ptr(), buf.len() as c_int) };
        if n == ffi::AVERROR_EOF {
            break;
        }
        if n < 0 {
            if crate::cancel::cancelled() {
                return Err(Error::Invalid(CANCELLED.into()));
            }
            return Err(ffmpeg_error(format!("downloading {}", f.url), n));
        }
        let n = n as usize;
        out.write_all(&buf[..n])?;
        hash.update(&buf[..n]);
        have += n as u64;
        if have > f.bytes {
            break;
        }
        crate::progress::report(
            target,
            Progress::stage(
                Stage::Downloading,
                have as i64,
                f.bytes as i64,
                started.elapsed(),
            ),
        );
    }
    out.flush()?;
    drop(out);
    let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if have != f.bytes || digest != f.sha256 {
        let _ = std::fs::remove_file(part);
        return Err(Error::Invalid(format!(
            "{}: downloaded {have} bytes with SHA-256 {digest}, expected {} bytes with {}",
            f.url, f.bytes, f.sha256
        )));
    }
    std::fs::rename(part, target)?;
    Ok(())
}
