use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, PoisonError};

use vidcut_core::editor::{TimelineFile, Unpacked};
use vidcut_core::{Error, ErrorCode, Project, Result};
use zip::write::SimpleFileOptions;

const CONTENT: &str = "content.otio";
const VERSION_FILE: &str = "version.txt";
const VERSION: &str = "1.0.0";
const MEDIA: &str = "media";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Otioz,
    Otiod,
}

impl Kind {
    pub fn of(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "otioz" => Some(Self::Otioz),
            "otiod" => Some(Self::Otiod),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Otioz => "otioz",
            Self::Otiod => "otiod",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub media: String,
    pub location: String,
    pub files: Vec<(PathBuf, String)>,
    pub bytes: u64,
}

fn io_error(path: &Path) -> impl Fn(std::io::Error) -> Error + '_ {
    move |e| Error::usage(format!("{}: {e}", path.display()))
}

fn zip_error(path: &Path) -> impl Fn(zip::result::ZipError) -> Error + '_ {
    move |e| match e {
        zip::result::ZipError::Io(e) => io_error(path)(e),
        e => Error::usage(format!("{}: {e}", path.display())),
    }
}

fn portable(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\u{FFFD}' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let s = s.trim().trim_end_matches('.');
    let s = match s.strip_prefix('.') {
        Some(rest) => format!("_{rest}"),
        None => s.to_string(),
    };
    if s.is_empty() { "media".into() } else { s }
}

fn unique(taken: &mut Vec<String>, name: &str) -> String {
    let (stem, ext) = match name.rfind('.').filter(|&i| i > 0) {
        Some(i) => (&name[..i], &name[i..]),
        None => (name, ""),
    };
    let mut n = 1;
    loop {
        let candidate = if n == 1 {
            name.to_string()
        } else {
            format!("{stem}-{n}{ext}")
        };
        if !taken.contains(&candidate.to_lowercase()) {
            taken.push(candidate.to_lowercase());
            return format!("{MEDIA}/{candidate}");
        }
        n += 1;
    }
}

pub fn plan(p: &Project, ids: &[String]) -> Result<Vec<Item>> {
    let mut taken = Vec::new();
    let mut items = Vec::new();
    let mut missing = Vec::new();
    let size = |path: &Path| {
        std::fs::metadata(path)
            .ok()
            .filter(|m| m.is_file())
            .map(|m| m.len())
    };
    for id in ids {
        let m = p
            .media(id)
            .ok_or_else(|| Error::internal(format!("no media {id}")))?;
        let mut item = Item {
            media: id.clone(),
            location: String::new(),
            files: Vec::new(),
            bytes: 0,
        };
        match &m.sequence {
            Some(s) => {
                let dir = m.path.parent().unwrap_or(Path::new("/"));
                let folder = dir
                    .file_name()
                    .map_or("pictures".into(), |n| portable(&n.to_string_lossy()));
                let folder = unique(&mut taken, &folder);
                for k in 0..s.count {
                    let file = s.file(dir, k);
                    match size(&file) {
                        Some(n) => {
                            item.bytes += n;
                            let name = s.name(s.first + k);
                            item.files.push((file, format!("{folder}/{name}")));
                        }
                        None => {
                            missing.push(file);
                            break;
                        }
                    }
                }
                item.location = format!("{folder}/");
            }
            None => match size(&m.path) {
                Some(n) => {
                    let name = m
                        .path
                        .file_name()
                        .map_or(m.name.clone(), |n| n.to_string_lossy().into_owned());
                    item.location = unique(&mut taken, &portable(&name));
                    item.bytes = n;
                    item.files.push((m.path.clone(), item.location.clone()));
                }
                None => missing.push(m.path.clone()),
            },
        }
        items.push(item);
    }
    if !missing.is_empty() {
        return Err(Error::media(format!(
            "{} media file(s) to bundle aren't there: {}",
            missing.len(),
            missing
                .iter()
                .map(|m| m.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    Ok(items)
}

fn copy(
    src: &Path,
    to: &mut dyn Write,
    done: &mut u64,
    total: u64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<()> {
    let mut f = File::open(src).map_err(io_error(src))?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::new(ErrorCode::Cancelled, "export cancelled"));
        }
        let n = f.read(&mut buf).map_err(io_error(src))?;
        if n == 0 {
            return Ok(());
        }
        to.write_all(&buf[..n])
            .map_err(|e| Error::usage(format!("writing the bundle: {e}")))?;
        *done += n as u64;
        progress(*done, total);
    }
}

pub fn write(
    kind: Kind,
    out: &Path,
    text: &str,
    items: &[Item],
    overwrite: bool,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<()> {
    if out.exists() {
        if !overwrite {
            return Err(Error::invalid(format!(
                "{} exists; pass overwrite to replace it",
                out.display()
            )));
        }
        if out.is_dir() && !(kind == Kind::Otiod && out.join(CONTENT).is_file()) {
            return Err(Error::invalid(format!(
                "{} is a folder, not an .otiod bundle: not replacing it",
                out.display()
            )));
        }
    }
    let part = vidcut_av::export::partial_path(out);
    let _ = std::fs::remove_dir_all(&part);
    let _ = std::fs::remove_file(&part);
    let total = items.iter().map(|i| i.bytes).sum();
    let mut done = 0;
    let written = match kind {
        Kind::Otioz => write_zip(&part, text, items, &mut done, total, cancel, progress),
        Kind::Otiod => write_folder(&part, text, items, &mut done, total, cancel, progress),
    };
    let moved = written.and_then(|()| {
        if kind == Kind::Otiod && out.is_dir() {
            std::fs::remove_dir_all(out).map_err(io_error(out))?;
        }
        std::fs::rename(&part, out).map_err(io_error(out))
    });
    if moved.is_err() {
        let _ = std::fs::remove_dir_all(&part);
        let _ = std::fs::remove_file(&part);
    }
    moved
}

fn write_zip(
    part: &Path,
    text: &str,
    items: &[Item],
    done: &mut u64,
    total: u64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<()> {
    let file = File::create(part).map_err(io_error(part))?;
    let mut zip = zip::ZipWriter::new(BufWriter::new(file));
    let err = zip_error(part);
    let werr = io_error(part);
    let options = SimpleFileOptions::default().unix_permissions(0o644);
    let deflated = options.compression_method(zip::CompressionMethod::Deflated);
    zip.start_file(VERSION_FILE, deflated).map_err(&err)?;
    zip.write_all(VERSION.as_bytes()).map_err(&werr)?;
    zip.start_file(CONTENT, deflated).map_err(&err)?;
    zip.write_all(text.as_bytes()).map_err(&werr)?;
    for (src, name) in items.iter().flat_map(|i| &i.files) {
        let size = std::fs::metadata(src).map_err(io_error(src))?.len();
        let stored = options
            .compression_method(zip::CompressionMethod::Stored)
            .large_file(size >= u64::from(u32::MAX));
        zip.start_file(name.as_str(), stored).map_err(&err)?;
        copy(src, &mut zip, done, total, cancel, progress)?;
    }
    zip.finish()
        .map_err(&err)?
        .into_inner()
        .map_err(|e| werr(e.into_error()))?
        .sync_all()
        .map_err(&werr)
}

fn write_folder(
    part: &Path,
    text: &str,
    items: &[Item],
    done: &mut u64,
    total: u64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<()> {
    std::fs::create_dir_all(part.join(MEDIA)).map_err(io_error(part))?;
    for (src, name) in items.iter().flat_map(|i| &i.files) {
        let to = part.join(name);
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir).map_err(io_error(dir))?;
        }
        let mut f = BufWriter::new(File::create(&to).map_err(io_error(&to))?);
        copy(src, &mut f, done, total, cancel, progress)?;
        f.flush().map_err(io_error(&to))?;
    }
    std::fs::write(part.join(CONTENT), text).map_err(io_error(part))
}

type Seen = (u64, i128, u32);

static CHECKED: LazyLock<Mutex<HashMap<PathBuf, Seen>>> = LazyLock::new(Default::default);

fn modified(meta: &std::fs::Metadata) -> i128 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as i128)
}

fn remember(path: &Path, crc: u32) {
    if let Ok(meta) = std::fs::metadata(path) {
        CHECKED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(path.to_path_buf(), (meta.len(), modified(&meta), crc));
    }
}

fn holds(path: &Path, meta: &std::fs::Metadata, crc: u32) -> bool {
    let known = CHECKED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(path)
        .copied();
    if known == Some((meta.len(), modified(meta), crc)) {
        return true;
    }
    let Ok(f) = File::open(path) else {
        return false;
    };
    let mut hasher = crc32fast::Hasher::new();
    let mut reader = BufReader::with_capacity(1 << 20, f);
    loop {
        let buf = match reader.fill_buf() {
            Ok([]) => break,
            Ok(b) => b,
            Err(_) => return false,
        };
        hasher.update(buf);
        let n = buf.len();
        reader.consume(n);
    }
    let same = hasher.finalize() == crc;
    if same {
        remember(path, crc);
    }
    same
}

pub fn open(path: &Path, unpack: Option<&Path>, write: bool) -> Result<TimelineFile> {
    let file = File::open(path).map_err(|e| {
        let message = format!("{}: {e}", path.display());
        match e.kind() {
            std::io::ErrorKind::NotFound => Error::not_found(message),
            _ => Error::usage(message),
        }
    })?;
    let mut zip = zip::ZipArchive::new(BufReader::new(file)).map_err(|e| {
        Error::usage(format!(
            "{}: not an .otioz bundle (a zip file): {e}",
            path.display()
        ))
    })?;
    let err = zip_error(path);
    let content = match zip.index_for_name(CONTENT) {
        Some(_) => CONTENT.to_string(),
        None => zip
            .file_names()
            .filter_map(|n| n.ok())
            .find(|n| !n.contains('/') && n.to_ascii_lowercase().ends_with(".otio"))
            .map(|n| n.into_owned())
            .ok_or_else(|| {
                Error::usage(format!("{}: no content.otio in the bundle", path.display()))
            })?,
    };
    let mut text = String::new();
    zip.by_name(&content)
        .map_err(&err)?
        .read_to_string(&mut text)
        .map_err(|e| Error::usage(format!("{}: {content}: {e}", path.display())))?;
    let folder = match unpack {
        Some(dir) => dir.to_path_buf(),
        None => path.with_extension(""),
    };
    let mut entries = Vec::new();
    for i in 0..zip.len() {
        let e = zip.by_index_raw(i).map_err(&err)?;
        let name = e.name().map_err(&err)?;
        if e.is_dir() || name == content || name == VERSION_FILE {
            continue;
        }
        let rel = e.enclosed_name().ok_or_else(|| {
            Error::usage(format!(
                "{}: {name:?} would unpack outside the bundle's folder",
                path.display(),
            ))
        })?;
        entries.push((i, rel, e.size(), e.crc32()));
    }
    let bytes = entries.iter().map(|e| e.2).sum();
    let mut unpacked = true;
    if write && !entries.is_empty() {
        std::fs::create_dir_all(&folder).map_err(io_error(&folder))?;
    }
    for (i, rel, size, crc) in &entries {
        let target = folder.join(rel);
        match std::fs::metadata(&target) {
            Ok(meta) => {
                let same = meta.is_file() && meta.len() == *size;
                if write && !(same && holds(&target, &meta, *crc)) {
                    return Err(Error::conflict(format!(
                        "{} is in the way of the bundle's {}: move it away, or unpack \
                         the bundle elsewhere",
                        target.display(),
                        rel.display()
                    )));
                }
                unpacked &= same;
            }
            Err(_) if !write => unpacked = false,
            Err(_) => {
                if let Some(dir) = target.parent() {
                    std::fs::create_dir_all(dir).map_err(io_error(dir))?;
                }
                let mut part = target.clone().into_os_string();
                part.push(".part");
                let part = PathBuf::from(part);
                let unpacked = (|| {
                    let mut out = BufWriter::new(File::create(&part).map_err(io_error(&part))?);
                    let mut entry = zip.by_index(*i).map_err(&err)?;
                    std::io::copy(&mut entry, &mut out).map_err(|e| {
                        Error::usage(format!("{}: {}: {e}", path.display(), rel.display()))
                    })?;
                    out.flush().map_err(io_error(&part))?;
                    std::fs::rename(&part, &target).map_err(io_error(&target))
                })();
                if unpacked.is_err() {
                    let _ = std::fs::remove_file(&part);
                }
                unpacked?;
                remember(&target, *crc);
            }
        }
    }
    Ok(TimelineFile {
        text,
        base: folder.clone(),
        bundle: Some(Unpacked {
            folder,
            files: entries.len(),
            bytes,
            unpacked,
        }),
    })
}
