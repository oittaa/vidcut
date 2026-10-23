use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, PoisonError};

use vidcut_core::editor::{TimelineFile, Unpacked};
use vidcut_core::model::ImageSequence;
use vidcut_core::{Error, ErrorCode, Project, Result};
use zip::write::SimpleFileOptions;

const CONTENT: &str = "content.otio";
const VERSION_FILE: &str = "version.txt";
const VERSION: &str = "1.0.0";
const MEDIA: &str = "media";
const CONTENT_MAX: u64 = 64 << 20;
const MEDIA_MAX: u64 = 64 << 40;

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
                let named = ImageSequence {
                    pattern: portable(&s.pattern),
                    ..s.clone()
                };
                for k in 0..s.count {
                    let file = s.file(dir, k);
                    match size(&file) {
                        Some(n) => {
                            item.bytes += n;
                            let name = named.name(named.first + k);
                            item.files.push((file, format!("{folder}/{name}")));
                        }
                        None => {
                            missing.push(file);
                            break;
                        }
                    }
                }
                item.location = format!("{folder}/{}", named.pattern);
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
    let part = match kind {
        Kind::Otioz => vidcut_core::temp::file(out).map(|(part, _)| part),
        Kind::Otiod => vidcut_core::temp::dir(out),
    }
    .map_err(io_error(out))?;
    let total = items.iter().map(|i| i.bytes).sum();
    let mut done = 0;
    match kind {
        Kind::Otioz => write_zip(part.path(), text, items, &mut done, total, cancel, progress),
        Kind::Otiod => write_folder(part.path(), text, items, &mut done, total, cancel, progress),
    }?;
    if kind == Kind::Otiod && out.is_dir() {
        replace_dir(part, out)
    } else {
        part.keep(out).map_err(io_error(out))
    }
}

fn replace_dir(part: vidcut_core::temp::Temp, out: &Path) -> Result<()> {
    let parent = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let stem: String = out.file_name().map_or_else(
        || "bundle".into(),
        |name| name.to_string_lossy().chars().take(48).collect(),
    );
    let mut n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut backup = None;
    for _ in 0..100 {
        let candidate = parent.join(format!(".{stem}.{n:x}.aside"));
        n = n.wrapping_add(1);
        if candidate.exists() {
            continue;
        }
        match std::fs::rename(out, &candidate) {
            Ok(()) => {
                backup = Some(candidate);
                break;
            }
            Err(_) if candidate.exists() => continue,
            Err(e) => return Err(io_error(out)(e)),
        }
    }
    let Some(backup) = backup else {
        return Err(Error::usage(format!(
            "{}: no place to keep the previous bundle",
            out.display()
        )));
    };
    match part.keep(out) {
        Ok(()) => {
            let _ = std::fs::remove_dir_all(&backup);
            Ok(())
        }
        Err(e) => match std::fs::rename(&backup, out) {
            Ok(()) => Err(io_error(out)(e)),
            Err(back) => Err(Error::usage(format!(
                "{}: {e}; the previous bundle is at {}: {back}",
                out.display(),
                backup.display()
            ))),
        },
    }
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
    {
        let entry = zip.by_name(&content).map_err(&err)?;
        let declared = entry.size();
        if declared > CONTENT_MAX {
            return Err(Error::usage(format!(
                "{}: {content} is larger than {} bytes",
                path.display(),
                CONTENT_MAX
            )));
        }
        let mut limited = entry.take(CONTENT_MAX.min(declared).saturating_add(1));
        limited
            .read_to_string(&mut text)
            .map_err(|e| Error::usage(format!("{}: {content}: {e}", path.display())))?;
        if text.len() as u64 != declared {
            return Err(Error::usage(format!(
                "{}: {content}: size mismatch (declared {declared}, got {})",
                path.display(),
                text.len()
            )));
        }
    }
    let folder = match unpack {
        Some(dir) => dir.to_path_buf(),
        None => path.with_extension(""),
    };
    let mut entries = Vec::new();
    for i in 0..zip.len() {
        let e = zip.by_index_raw(i).map_err(&err)?;
        let name = e.name().map_err(&err)?;
        if e.is_symlink() {
            return Err(Error::usage(format!(
                "{}: {name:?} is a symbolic link",
                path.display()
            )));
        }
        if e.is_dir() || name == content || name == VERSION_FILE {
            continue;
        }
        let rel = e.enclosed_name().ok_or_else(|| {
            Error::usage(format!(
                "{}: {name:?} would unpack outside the bundle's folder",
                path.display(),
            ))
        })?;
        let size = e.size();
        if size > MEDIA_MAX {
            return Err(Error::usage(format!(
                "{}: {name:?} is larger than {MEDIA_MAX} bytes",
                path.display()
            )));
        }
        entries.push((i, rel, size, e.crc32()));
    }
    let mut bytes = 0u64;
    for (_, _, size, _) in &entries {
        bytes = match bytes.checked_add(*size) {
            Some(n) if n <= MEDIA_MAX => n,
            _ => {
                return Err(Error::usage(format!(
                    "{}: the bundle's media are larger than {MEDIA_MAX} bytes",
                    path.display()
                )));
            }
        };
    }
    let mut unpacked = true;
    if write && !entries.is_empty() {
        std::fs::create_dir_all(&folder).map_err(io_error(&folder))?;
    }
    for (i, rel, size, crc) in &entries {
        let target = folder.join(rel);
        match std::fs::symlink_metadata(&target) {
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
                let (part, file) = vidcut_core::temp::file(&target).map_err(io_error(&target))?;
                let mut out = BufWriter::new(file);
                let entry = zip.by_index(*i).map_err(&err)?;
                let mut limited = entry.take(size.saturating_add(1));
                let copied = std::io::copy(&mut limited, &mut out).map_err(|e| {
                    Error::usage(format!("{}: {}: {e}", path.display(), rel.display()))
                })?;
                if copied != *size {
                    return Err(Error::usage(format!(
                        "{}: {}: size mismatch (declared {size}, got {copied})",
                        path.display(),
                        rel.display()
                    )));
                }
                out.flush().map_err(io_error(part.path()))?;
                drop(out);
                part.keep(&target).map_err(io_error(&target))?;
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

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::Path;
    use std::sync::atomic::AtomicBool;

    use vidcut_core::model::{ImageSequence, Media, Settings};
    use vidcut_core::{ErrorCode, Project};
    use zip::CompressionMethod;
    use zip::write::SimpleFileOptions;

    use super::{Kind, open, plan, write};

    const TEXT: &[u8] = b"{\"ok\":true}";
    const MEDIA_BYTES: &[u8] = b"abcd";

    fn media(path: &Path, sequence: Option<ImageSequence>) -> Media {
        let mut item: Media = serde_json::from_value(serde_json::json!({
            "id": "m1",
            "path": "/tmp/m1",
            "name": "m1",
            "size": 1,
            "mtime_ns": 0,
            "info": {
                "format": "png",
                "duration": 0.04,
                "start_time": 0.0,
                "video": null,
                "audio": []
            },
        }))
        .unwrap();
        item.path = path.to_path_buf();
        item.sequence = sequence;
        item
    }

    fn project(item: Media) -> Project {
        let mut p = Project::new(Settings::default());
        p.media.push(item);
        p
    }

    fn stored(path: &Path, files: &[(&str, &[u8])], link: Option<(&str, &str)>) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .large_file(true);
        for (name, bytes) in files {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(bytes).unwrap();
        }
        if let Some((name, target)) = link {
            zip.add_symlink(name, target, opts).unwrap();
        }
        zip.finish().unwrap();
    }

    fn claim(path: &Path, actual: u64, claimed: u64) {
        let mut buf = std::fs::read(path).unwrap();
        let needle = actual.to_le_bytes();
        let mut found = 0;
        let mut i = 0;
        while i + 16 <= buf.len() {
            if buf[i..i + 8] == needle && buf[i + 8..i + 16] == needle {
                buf[i..i + 8].copy_from_slice(&claimed.to_le_bytes());
                found += 1;
                i += 16;
            } else {
                i += 1;
            }
        }
        assert!(found >= 2);
        std::fs::write(path, buf).unwrap();
        let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let size = zip
            .by_name(if actual == MEDIA_BYTES.len() as u64 {
                "media/a.bin"
            } else {
                "content.otio"
            })
            .unwrap()
            .size();
        assert_eq!(size, claimed);
    }

    fn refuse(path: &Path) -> ErrorCode {
        let unpack = path.parent().unwrap().join("unpack");
        let result = open(path, Some(&unpack), true);
        assert!(
            !unpack.join("media/a.bin").exists(),
            "wrote media from a bundle that should be refused"
        );
        assert!(
            !unpack.join("media/link").exists(),
            "wrote a symbolic link from the bundle"
        );
        result.unwrap_err().code
    }

    #[test]
    fn sequence_frame_names_stay_in_one_path_segment() {
        let root = tempfile::tempdir().unwrap();
        let pics = root.path().join("pics");
        std::fs::create_dir(&pics).unwrap();
        let sequence = ImageSequence {
            pattern: r"../evil_%d.png".into(),
            first: 1,
            count: 1,
        };
        std::fs::write(sequence.file(&pics, 0), b"p").unwrap();
        let items = plan(
            &project(media(&pics.join("first.png"), Some(sequence))),
            &["m1".into()],
        )
        .unwrap();
        assert_eq!(items[0].files[0].1, "media/pics/_._evil_1.png");
    }

    #[test]
    fn a_plain_sequence_frame_name_is_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let pics = root.path().join("pics");
        std::fs::create_dir(&pics).unwrap();
        let sequence = ImageSequence {
            pattern: "frame_%04d.png".into(),
            first: 1,
            count: 1,
        };
        std::fs::write(sequence.file(&pics, 0), b"p").unwrap();
        let items = plan(
            &project(media(&pics.join("first.png"), Some(sequence))),
            &["m1".into()],
        )
        .unwrap();
        assert_eq!(items[0].files[0].1, "media/pics/frame_0001.png");
    }

    #[test]
    fn open_reads_a_bundle_it_wrote() {
        let root = tempfile::tempdir().unwrap();
        let src = root.path().join("clip.mov");
        std::fs::write(&src, MEDIA_BYTES).unwrap();
        let items = plan(&project(media(&src, None)), &["m1".into()]).unwrap();
        let out = root.path().join("cut.otioz");
        let cancel = AtomicBool::new(false);
        write(
            Kind::Otioz,
            &out,
            "{\"ok\":true}",
            &items,
            false,
            &cancel,
            &mut |_, _| {},
        )
        .unwrap();
        let unpack = root.path().join("unpack");
        let file = open(&out, Some(&unpack), true).unwrap();
        assert_eq!(file.text, "{\"ok\":true}");
        assert_eq!(
            std::fs::read(unpack.join("media/clip.mov")).unwrap(),
            MEDIA_BYTES
        );
    }

    #[test]
    fn open_refuses_a_symbolic_link() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("cut.otioz");
        stored(
            &path,
            &[("content.otio", TEXT)],
            Some(("media/link", "/etc/passwd")),
        );
        assert_eq!(refuse(&path), ErrorCode::Usage);
    }

    #[test]
    fn open_refuses_media_whose_length_differs() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("cut.otioz");
        stored(
            &path,
            &[("content.otio", TEXT), ("media/a.bin", MEDIA_BYTES)],
            None,
        );
        claim(&path, MEDIA_BYTES.len() as u64, 100);
        assert_eq!(refuse(&path), ErrorCode::Usage);
    }

    #[test]
    fn open_refuses_media_that_declare_an_absurd_size() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("cut.otioz");
        stored(
            &path,
            &[("content.otio", TEXT), ("media/a.bin", MEDIA_BYTES)],
            None,
        );
        claim(&path, MEDIA_BYTES.len() as u64, (64u64 << 40) + 1);
        assert_eq!(refuse(&path), ErrorCode::Usage);
    }

    #[test]
    fn open_refuses_timeline_text_over_64_mib() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("cut.otioz");
        stored(&path, &[("content.otio", TEXT)], None);
        claim(&path, TEXT.len() as u64, (64u64 << 20) + 1);
        assert_eq!(refuse(&path), ErrorCode::Usage);
    }

    #[test]
    fn open_refuses_timeline_text_whose_length_differs() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("cut.otioz");
        stored(&path, &[("content.otio", TEXT)], None);
        claim(&path, TEXT.len() as u64, 100);
        assert_eq!(refuse(&path), ErrorCode::Usage);
    }

    #[test]
    fn replacing_an_otiod_keeps_the_new_text_and_a_sibling() {
        let root = tempfile::tempdir().unwrap();
        let out = root.path().join("cut.otiod");
        let cancel = AtomicBool::new(false);
        let mut progress = |_: u64, _: u64| {};
        write(Kind::Otiod, &out, "one", &[], false, &cancel, &mut progress).unwrap();
        std::fs::write(root.path().join(".cut.otiod.old"), b"mine").unwrap();
        std::fs::write(root.path().join("cut.otiod-prev"), b"prev").unwrap();
        write(Kind::Otiod, &out, "two", &[], true, &cancel, &mut progress).unwrap();
        assert_eq!(std::fs::read(out.join(super::CONTENT)).unwrap(), b"two");
        assert_eq!(
            std::fs::read(root.path().join(".cut.otiod.old")).unwrap(),
            b"mine"
        );
        assert_eq!(
            std::fs::read(root.path().join("cut.otiod-prev")).unwrap(),
            b"prev"
        );
    }

    #[test]
    fn a_failed_otiod_replace_puts_the_old_folder_back() {
        let root = tempfile::tempdir().unwrap();
        let out = root.path().join("cut.otiod");
        std::fs::create_dir(&out).unwrap();
        std::fs::write(out.join(super::CONTENT), b"old").unwrap();
        let part = vidcut_core::temp::dir(&out).unwrap();
        std::fs::remove_dir(part.path()).unwrap();
        let err = super::replace_dir(part, &out).unwrap_err();
        assert_eq!(err.code, ErrorCode::Usage);
        assert_eq!(std::fs::read(out.join(super::CONTENT)).unwrap(), b"old");
        let mut names: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(names, vec![std::ffi::OsString::from("cut.otiod")]);
    }

    #[cfg(unix)]
    #[test]
    fn open_does_not_write_through_a_symlink() {
        let root = tempfile::tempdir().unwrap();
        let src = root.path().join("clip.mov");
        std::fs::write(&src, MEDIA_BYTES).unwrap();
        let items = plan(&project(media(&src, None)), &["m1".into()]).unwrap();
        let out = root.path().join("cut.otioz");
        let cancel = AtomicBool::new(false);
        write(
            Kind::Otioz,
            &out,
            "{\"ok\":true}",
            &items,
            false,
            &cancel,
            &mut |_, _| {},
        )
        .unwrap();
        let unpack = root.path().join("unpack");
        let media_dir = unpack.join("media");
        std::fs::create_dir_all(&media_dir).unwrap();
        let outside = root.path().join("outside");
        std::os::unix::fs::symlink(&outside, media_dir.join("clip.mov")).unwrap();
        let err = open(&out, Some(&unpack), true).unwrap_err();
        assert_eq!(err.code, ErrorCode::Conflict);
        assert!(!outside.exists());
    }
}
