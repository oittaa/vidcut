use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, Transaction, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::change::Change;
use crate::editor::{Editor, Entry, History, Moved};
use crate::error::{Error, Result};
use crate::history::{self, HistoryDb, HistoryMeta, compress_bytes, decompress_bytes};
use crate::lru::Lru;
use crate::model::{
    Clip, ClipFilter, Compound, Cue, Grade, Kept, Marker, Marks, Media, NextIds, OpenCompound,
    PathSample, Project, RenderJob, RenderRange, Settings, Speakers, SubtitleStyle, SubtitleTrack,
    SyncGroup, TapeMarks, Timeline, TimelineSettings, Tool, Track, TrackKind, Transition,
    VoiceIsolation,
};
use crate::shared::SharedVec;

const APP_ID: i32 = 0x5643_5554;
const SCHEMA_VERSION: i32 = 1;
const BLOB_MIN_SAMPLES: usize = 16;
const BLOB_HASHES_KEPT: usize = 512;
const BUSY_TIMEOUT: Duration = Duration::from_secs(30);

enum BlobWeak {
    Samples(Weak<Vec<PathSample>>),
    Frames(Weak<Vec<[f32; 8]>>),
}

type BlobHashCache = Mutex<Lru<usize, (BlobWeak, String), BLOB_HASHES_KEPT>>;

const SCHEMA: &str = "\
PRAGMA page_size = 4096;
PRAGMA application_id = 1447253332;
PRAGMA user_version = 1;
CREATE TABLE meta (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    rev INTEGER NOT NULL,
    edited INTEGER,
    data TEXT NOT NULL
);
CREATE TABLE media (
    id TEXT PRIMARY KEY,
    ord INTEGER NOT NULL,
    path TEXT NOT NULL,
    name TEXT NOT NULL,
    bin TEXT NOT NULL,
    data TEXT NOT NULL
);
CREATE TABLE timelines (
    id TEXT PRIMARY KEY,
    ord INTEGER NOT NULL,
    name TEXT NOT NULL,
    bin TEXT NOT NULL,
    data TEXT NOT NULL
);
CREATE TABLE tracks (
    owner TEXT NOT NULL,
    id TEXT NOT NULL,
    ord INTEGER NOT NULL,
    kind TEXT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (owner, id)
);
CREATE TABLE clips (
    owner TEXT NOT NULL,
    track_id TEXT NOT NULL,
    id TEXT NOT NULL,
    ord INTEGER NOT NULL,
    start INTEGER NOT NULL,
    duration INTEGER NOT NULL,
    media_id TEXT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (owner, track_id, id)
);
CREATE TABLE subtitles (
    tl_id TEXT NOT NULL,
    id TEXT NOT NULL,
    ord INTEGER NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (tl_id, id)
);
CREATE TABLE cues (
    tl_id TEXT NOT NULL,
    sub_id TEXT NOT NULL,
    id TEXT NOT NULL,
    ord INTEGER NOT NULL,
    start INTEGER NOT NULL,
    duration INTEGER NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (tl_id, sub_id, id)
);
CREATE TABLE compounds (
    id TEXT PRIMARY KEY,
    ord INTEGER NOT NULL,
    name TEXT NOT NULL,
    data TEXT NOT NULL
);
CREATE TABLE blobs (
    hash TEXT PRIMARY KEY,
    data BLOB NOT NULL
);
CREATE TABLE history_meta (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    undo_count INTEGER NOT NULL,
    redo_count INTEGER NOT NULL,
    total_steps INTEGER NOT NULL,
    next_step_id INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    external_dirty INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE history_base (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    base_json BLOB
);
CREATE TABLE history_events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    kind INTEGER NOT NULL,
    at_pos INTEGER,
    step_id INTEGER,
    label TEXT,
    step_meta TEXT,
    change_delta BLOB
);
CREATE INDEX idx_history_events_kind_at ON history_events (kind, at_pos, seq DESC);
CREATE TRIGGER tr_meta_i AFTER INSERT ON meta BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_meta_u AFTER UPDATE ON meta BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_meta_d AFTER DELETE ON meta BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_media_i AFTER INSERT ON media BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_media_u AFTER UPDATE ON media BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_media_d AFTER DELETE ON media BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_timelines_i AFTER INSERT ON timelines BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_timelines_u AFTER UPDATE ON timelines BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_timelines_d AFTER DELETE ON timelines BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_tracks_i AFTER INSERT ON tracks BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_tracks_u AFTER UPDATE ON tracks BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_tracks_d AFTER DELETE ON tracks BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_clips_i AFTER INSERT ON clips BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_clips_u AFTER UPDATE ON clips BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_clips_d AFTER DELETE ON clips BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_subtitles_i AFTER INSERT ON subtitles BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_subtitles_u AFTER UPDATE ON subtitles BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_subtitles_d AFTER DELETE ON subtitles BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_cues_i AFTER INSERT ON cues BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_cues_u AFTER UPDATE ON cues BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_cues_d AFTER DELETE ON cues BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_compounds_i AFTER INSERT ON compounds BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_compounds_u AFTER UPDATE ON compounds BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
CREATE TRIGGER tr_compounds_d AFTER DELETE ON compounds BEGIN UPDATE history_meta SET external_dirty = 1 WHERE id = 1 AND external_dirty = 0; END;
";

pub fn state_dir() -> PathBuf {
    crate::dirs::state()
}

#[derive(Debug, Clone)]
pub struct Saved {
    pub compact: bool,
}

pub struct LiveOutcome<T> {
    pub result: Result<T>,
    pub refreshed: bool,
    pub committed: bool,
    pub saved: Option<Result<Saved>>,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileId(u64, u64);

#[cfg(not(unix))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileId;

#[cfg(unix)]
fn file_id_of(path: &Path) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some(FileId(meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn file_id_of(path: &Path) -> Option<FileId> {
    path.exists().then_some(FileId)
}

pub struct Store {
    path: PathBuf,
    conn: Mutex<Option<Connection>>,
    blob_hashes: BlobHashCache,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").field("path", &self.path).finish()
    }
}

impl Clone for Store {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            conn: Mutex::new(None),
            blob_hashes: Mutex::new(Lru::new()),
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq)]
struct MetaRow<'a> {
    settings: Cow<'a, Settings>,
    recent: Cow<'a, [String]>,
    compound_path: Cow<'a, [OpenCompound]>,
    tape_marks: Cow<'a, TapeMarks>,
    next_id: Cow<'a, NextIds>,
    bins: Cow<'a, [String]>,
    groups: Cow<'a, [SyncGroup]>,
    speakers: Cow<'a, Speakers>,
    queue: Cow<'a, [RenderJob]>,
}

impl<'a> MetaRow<'a> {
    fn of(p: &'a Project) -> Self {
        let Project {
            rev: _,
            edited: _,
            settings,
            media: _,
            tracks: _,
            compounds: _,
            compound_path,
            marks: _,
            tape_marks,
            markers: _,
            subtitles: _,
            timelines: _,
            recent,
            next_id,
            bins,
            groups,
            speakers,
            queue,
        } = p;
        Self {
            settings: Cow::Borrowed(settings),
            recent: Cow::Borrowed(recent.as_slice()),
            compound_path: Cow::Borrowed(compound_path.as_slice()),
            tape_marks: Cow::Borrowed(tape_marks),
            next_id: Cow::Borrowed(next_id),
            bins: Cow::Borrowed(bins.as_slice()),
            groups: Cow::Borrowed(groups.as_slice()),
            speakers: Cow::Borrowed(speakers),
            queue: Cow::Borrowed(queue.as_slice()),
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq)]
struct TimelineRow<'a> {
    id: Cow<'a, str>,
    name: Cow<'a, str>,
    bin: Cow<'a, str>,
    start_tc: i64,
    own: Option<TimelineSettings>,
    grade: Cow<'a, Option<Grade>>,
    render: RenderRange,
    clip_filter: Cow<'a, ClipFilter>,
    marks: Cow<'a, Marks>,
    markers: Cow<'a, [Marker]>,
}

impl<'a> TimelineRow<'a> {
    fn of(p: &'a Project, t: &'a Timeline) -> Self {
        let Timeline {
            id,
            name,
            bin,
            start_tc,
            own,
            grade,
            render,
            clip_filter,
            tracks: _,
            marks: _,
            markers: _,
            subtitles: _,
        } = t;
        let (_, marks, markers, _) = timeline_sheet(p, t);
        Self {
            id: Cow::Borrowed(id.as_str()),
            name: Cow::Borrowed(name.as_str()),
            bin: Cow::Borrowed(bin.as_str()),
            start_tc: *start_tc,
            own: *own,
            grade: Cow::Borrowed(grade),
            render: *render,
            clip_filter: Cow::Borrowed(clip_filter),
            marks: Cow::Borrowed(marks),
            markers: Cow::Borrowed(markers.as_slice()),
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq)]
struct CompoundRow<'a> {
    id: Cow<'a, str>,
    name: Cow<'a, str>,
    bin: Cow<'a, str>,
    removed: bool,
    sequence_id: Cow<'a, str>,
    fps: crate::time::Rational,
    drop_frame: bool,
    start_tc: i64,
    frames: i64,
    marks: Cow<'a, Marks>,
    markers: Cow<'a, [Marker]>,
    kept_otio: Cow<'a, Option<Kept>>,
}

impl<'a> CompoundRow<'a> {
    fn of(p: &'a Project, k: &'a Compound) -> Self {
        let Compound {
            id,
            name,
            bin,
            removed,
            sequence_id,
            fps,
            drop_frame,
            start_tc,
            frames,
            tracks: _,
            marks: _,
            markers: _,
            kept_otio,
        } = k;
        let (_, marks, markers) = compound_sheet(p, k);
        Self {
            id: Cow::Borrowed(id.as_str()),
            name: Cow::Borrowed(name.as_str()),
            bin: Cow::Borrowed(bin.as_str()),
            removed: *removed,
            sequence_id: Cow::Borrowed(sequence_id.as_str()),
            fps: *fps,
            drop_frame: *drop_frame,
            start_tc: *start_tc,
            frames: *frames,
            marks: Cow::Borrowed(marks),
            markers: Cow::Borrowed(markers.as_slice()),
            kept_otio: Cow::Borrowed(kept_otio),
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq)]
struct TrackRow<'a> {
    id: Cow<'a, str>,
    kind: TrackKind,
    locked: bool,
    muted: bool,
    solo: bool,
    volume_db: f64,
    voice_isolation: Option<VoiceIsolation>,
    duck: Cow<'a, Option<crate::model::Duck>>,
    equalizer: Cow<'a, Option<crate::model::TrackEqualizer>>,
    dynamics: Option<crate::model::TrackDynamics>,
    pan: f64,
    pan_on: bool,
    video_enabled: bool,
    name: Cow<'a, Option<String>>,
    transitions: Cow<'a, [Transition]>,
    render_cache: bool,
}

impl<'a> TrackRow<'a> {
    fn of(t: &'a Track) -> Self {
        let Track {
            id,
            kind,
            clips: _,
            locked,
            muted,
            solo,
            volume_db,
            voice_isolation,
            duck,
            equalizer,
            dynamics,
            pan,
            pan_on,
            video_enabled,
            name,
            transitions,
            render_cache,
        } = t;
        Self {
            id: Cow::Borrowed(id.as_str()),
            kind: *kind,
            locked: *locked,
            muted: *muted,
            solo: *solo,
            volume_db: *volume_db,
            voice_isolation: *voice_isolation,
            duck: Cow::Borrowed(duck),
            equalizer: Cow::Borrowed(equalizer),
            dynamics: *dynamics,
            pan: *pan,
            pan_on: *pan_on,
            video_enabled: *video_enabled,
            name: Cow::Borrowed(name),
            transitions: Cow::Borrowed(transitions.as_slice()),
            render_cache: *render_cache,
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq)]
struct SubtitleTrackRow<'a> {
    id: Cow<'a, str>,
    name: Cow<'a, Option<String>>,
    language: Cow<'a, Option<String>>,
    shown: bool,
    destination: bool,
    locked: bool,
    style: Cow<'a, SubtitleStyle>,
}

impl<'a> SubtitleTrackRow<'a> {
    fn of(s: &'a SubtitleTrack) -> Self {
        let SubtitleTrack {
            id,
            name,
            language,
            shown,
            destination,
            locked,
            style,
            cues: _,
        } = s;
        Self {
            id: Cow::Borrowed(id.as_str()),
            name: Cow::Borrowed(name),
            language: Cow::Borrowed(language),
            shown: *shown,
            destination: *destination,
            locked: *locked,
            style: Cow::Borrowed(style),
        }
    }
}

fn configure_journal(conn: &Connection, remote: bool, writable: bool) -> rusqlite::Result<()> {
    if writable {
        let want = if remote { "delete" } else { "wal" };
        let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case(want) {
            let pragma = if remote {
                "PRAGMA journal_mode = DELETE;"
            } else {
                "PRAGMA journal_mode = WAL;"
            };
            conn.execute_batch(pragma)?;
        }
    }
    conn.execute_batch("PRAGMA synchronous = FULL; PRAGMA temp_store = MEMORY;")
}

pub(crate) fn settle(conn: &Connection) {
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE);");
}

fn configure_conn(conn: &Connection, path: &Path, writable: bool) -> rusqlite::Result<()> {
    configure_journal(conn, crate::dirs::on_network(path), writable)
}

fn verify_db(conn: &Connection, path: &Path) -> Result<()> {
    let invalid = || Error::invalid(format!("{} is not a vidcut project", path.display()));
    let app_id: i32 = conn
        .query_row("PRAGMA application_id", [], |r| r.get(0))
        .map_err(|_| invalid())?;
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|_| invalid())?;
    if app_id != APP_ID || version != SCHEMA_VERSION {
        return Err(invalid());
    }
    Ok(())
}

fn open_with(path: &Path, flags: OpenFlags, writable: bool) -> Result<Connection> {
    let conn = Connection::open_with_flags(path, flags | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|e| open_err(path, e))?;
    conn.busy_timeout(BUSY_TIMEOUT)
        .map_err(|e| open_err(path, e))?;
    verify_db(&conn, path)?;
    configure_conn(&conn, path, writable).map_err(|e| open_err(path, e))?;
    Ok(conn)
}

pub(crate) fn open_conn(path: &Path) -> Result<Connection> {
    open_with(path, OpenFlags::SQLITE_OPEN_READ_WRITE, true)
}

fn open_ro_or_rw(path: &Path) -> Result<Connection> {
    open_conn(path).or_else(|_| open_with(path, OpenFlags::SQLITE_OPEN_READ_ONLY, false))
}

fn query_data_version(conn: &Connection, path: &Path) -> Result<i64> {
    conn.query_row("PRAGMA data_version", [], |r| r.get(0))
        .map_err(|e| open_err(path, e))
}

fn read_tx<T>(
    conn: &mut Connection,
    path: &Path,
    f: impl FnOnce(&Connection) -> Result<T>,
) -> Result<T> {
    let tx = conn.transaction().map_err(|e| sqlite_err(path, e))?;
    let out = f(&tx)?;
    tx.commit().map_err(|e| sqlite_err(path, e))?;
    Ok(out)
}

fn open_err(path: &Path, e: rusqlite::Error) -> Error {
    match e.sqlite_error_code() {
        Some(rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt) => {
            Error::invalid(format!("{} is not a vidcut project: {e}", path.display()))
        }
        _ => sqlite_err(path, e),
    }
}

fn sqlite_err(path: &Path, e: rusqlite::Error) -> Error {
    Error::internal(format!("{}: {e}", path.display()))
}

fn json_err(e: serde_json::Error) -> Error {
    Error::internal(e.to_string())
}

fn io(path: &Path, e: std::io::Error) -> Error {
    Error::internal(format!("{}: {e}", path.display()))
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut os = path.as_os_str().to_owned();
    os.push(suffix);
    PathBuf::from(os)
}

struct SaveCommit {
    meta: HistoryMeta,
    added: Vec<history::Added>,
}

impl Store {
    pub fn create(path: &Path, project: &Project) -> Result<Self> {
        if path.exists() {
            return Err(Error::invalid(format!("{} already exists", path.display())));
        }
        Self::replace(path, project)
    }

    pub fn replace(path: &Path, project: &Project) -> Result<Self> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent).map_err(|e| io(path, e))?;
        let dir = crate::dirs::canonical(parent).map_err(|e| io(path, e))?;
        let (temp, handle) = crate::temp::file(path).map_err(|e| io(path, e))?;
        drop(handle);
        {
            let mut conn = Connection::open_with_flags(
                temp.path(),
                OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_CREATE
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(|e| sqlite_err(temp.path(), e))?;
            conn.execute_batch(SCHEMA)
                .map_err(|e| sqlite_err(temp.path(), e))?;
            configure_conn(&conn, temp.path(), true).map_err(|e| sqlite_err(temp.path(), e))?;
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|e| sqlite_err(temp.path(), e))?;
            sync_project(&tx, &dir, None, project, None)?;
            let mut rel = project.clone();
            relativize_project(&mut rel, &dir);
            HistoryDb::init_meta_in(&tx, &rel)?;
            clear_dirty(&tx, temp.path())?;
            tx.commit().map_err(|e| sqlite_err(temp.path(), e))?;
            if !crate::dirs::on_network(temp.path()) {
                conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                    .map_err(|e| sqlite_err(temp.path(), e))?;
            }
        }
        let _ = std::fs::remove_file(sidecar(temp.path(), "-wal"));
        let _ = std::fs::remove_file(sidecar(temp.path(), "-shm"));
        let _ = std::fs::remove_file(sidecar(path, "-wal"));
        let _ = std::fs::remove_file(sidecar(path, "-shm"));
        temp.keep(path).map_err(|e| io(path, e))?;
        Self::open(path)
    }

    pub fn open(path: &Path) -> Result<Self> {
        let path = crate::dirs::canonical(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::not_found(format!("no project file {}", path.display()))
            } else {
                io(path, e)
            }
        })?;
        Ok(Self {
            path,
            conn: Mutex::new(None),
            blob_hashes: Mutex::new(Lru::new()),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn dir(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new("/"))
    }

    fn history_db(&self) -> HistoryDb {
        HistoryDb::new(self.path.clone())
    }

    pub fn close(&self) {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let Some(mut conn) = guard.take() else {
            return;
        };
        if let Ok(tx) = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            && gc_blobs(&tx).is_ok()
        {
            let _ = tx.commit();
        }
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }

    fn with_conn<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(open_conn(&self.path)?);
        }
        let conn = guard.as_mut().expect("connection opened");
        f(conn)
    }

    fn live_conn<'a>(
        &self,
        guard: &'a mut Option<Connection>,
        last_id: &mut Option<FileId>,
        last_dv: &mut Option<i64>,
    ) -> Result<&'a mut Connection> {
        let current_id = file_id_of(&self.path);
        if *last_id != current_id {
            *guard = None;
            *last_id = current_id;
        }
        if guard.is_none() {
            *guard = Some(open_conn(&self.path)?);
            *last_dv = None;
        }
        Ok(guard.as_mut().expect("connection opened"))
    }

    pub fn reload_if_changed(
        &self,
        last_id: &mut Option<FileId>,
        last_dv: &mut Option<i64>,
    ) -> Result<Option<Editor>> {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let conn = self.live_conn(&mut guard, last_id, last_dv)?;
        let read = read_tx(conn, &self.path, |tx| {
            let dv = query_data_version(tx, &self.path)?;
            if *last_dv == Some(dv) {
                return Ok(None);
            }
            Ok(Some((self.load_in(tx)?, dv)))
        })?;
        Ok(read.map(|(editor, dv)| {
            *last_dv = Some(dv);
            editor
        }))
    }

    pub fn load_live(
        &self,
        last_id: &mut Option<FileId>,
        last_dv: &mut Option<i64>,
    ) -> Result<Editor> {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let conn = self.live_conn(&mut guard, last_id, last_dv)?;
        let (editor, dv) = read_tx(conn, &self.path, |tx| {
            let dv = query_data_version(tx, &self.path)?;
            Ok((self.load_in(tx)?, dv))
        })?;
        *last_dv = Some(dv);
        Ok(editor)
    }

    pub fn mutate_live<T>(
        &self,
        last_id: &mut Option<FileId>,
        last_dv: &mut Option<i64>,
        editor: &mut Editor,
        opened: u64,
        apply: impl FnOnce(&mut Editor) -> Result<T>,
    ) -> Result<LiveOutcome<T>> {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let conn = self.live_conn(&mut guard, last_id, last_dv)?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| sqlite_err(&self.path, e))?;
        let dv = query_data_version(&tx, &self.path)?;
        let mut refreshed = false;
        if *last_dv != Some(dv) {
            let mut loaded = self.load_in(&tx)?;
            loaded.opened_at(opened);
            *editor = loaded;
            *last_dv = Some(dv);
            refreshed = true;
        }
        let was = editor.unsaved();
        let result = apply(editor);
        let committed = editor.unsaved() != was;
        let saved = if committed {
            let outcome = self.save_in_tx(&tx, editor).and_then(|(s, commit)| {
                tx.commit().map_err(|e| sqlite_err(&self.path, e))?;
                editor.commit_saved(
                    self.history_db(),
                    (commit.meta.undo, commit.meta.redo),
                    commit.meta.generation,
                    editor.project().clone(),
                    commit.added,
                );
                Ok(s)
            });
            if outcome.is_ok() {
                *last_dv = Some(query_data_version(conn, &self.path)?);
                settle(conn);
            }
            Some(outcome)
        } else {
            None
        };
        Ok(LiveOutcome {
            result,
            refreshed,
            committed,
            saved,
        })
    }

    pub fn load(&self) -> Result<Editor> {
        self.with_conn(|conn| read_tx(conn, &self.path, |tx| self.load_in(tx)))
    }

    fn load_in(&self, conn: &Connection) -> Result<Editor> {
        let project =
            load_project_from_conn(conn, &self.path, self.dir(), Some(&self.blob_hashes))?;
        let meta = HistoryDb::read_meta_in(conn)?;
        if !meta.external_dirty {
            return Ok(Editor::on_db(
                project.clone(),
                self.history_db(),
                (meta.undo, meta.redo),
                meta.generation,
                Some(project),
            ));
        }
        let Some(mut last) = HistoryDb::last_saved_in(conn, self.dir()) else {
            return Ok(Editor::on_db(
                project,
                self.history_db(),
                (meta.undo, meta.redo),
                meta.generation,
                None,
            ));
        };
        for job in &mut last.queue {
            if let Some(now) = project.queue.iter().find(|j| j.id == job.id) {
                job.status.clone_from(&now.status);
            }
        }
        let mut editor = Editor::on_db(
            last,
            self.history_db(),
            (meta.undo, meta.redo),
            meta.generation,
            None,
        );
        editor.record_external(project);
        Ok(editor)
    }

    pub fn load_project(&self) -> Result<Editor> {
        let mut conn = open_ro_or_rw(&self.path)?;
        let (project, meta) = read_tx(&mut conn, &self.path, |tx| {
            let project =
                load_project_from_conn(tx, &self.path, self.dir(), Some(&self.blob_hashes))?;
            Ok((project, HistoryDb::read_meta_in(tx)?))
        })?;
        Ok(Editor::on_db(
            project.clone(),
            self.history_db(),
            (meta.undo, meta.redo),
            meta.generation,
            Some(project),
        ))
    }

    pub fn project_at(
        &self,
        past: &mut crate::past::Past,
        current: Project,
        step: usize,
    ) -> Result<std::sync::Arc<Project>> {
        let db = self.history_db();
        let meta = db.meta()?;
        past.project_at(
            &Editor::on_db(current, db, (meta.undo, meta.redo), meta.generation, None),
            step,
        )
    }

    pub fn version_at(
        &self,
        past: &mut crate::past::Past,
        current: Project,
        id: u64,
        k: usize,
    ) -> Result<std::sync::Arc<Project>> {
        let db = self.history_db();
        let meta = db.meta()?;
        past.version_at(
            &Editor::on_db(current, db, (meta.undo, meta.redo), meta.generation, None),
            id,
            k,
        )
    }

    pub fn transact<T>(&self, f: impl FnOnce(&mut Editor) -> Result<T>) -> Result<T> {
        let (out, compact) = {
            let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            if guard.is_none() {
                *guard = Some(open_conn(&self.path)?);
            }
            let conn = guard.as_mut().expect("connection opened");
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|e| sqlite_err(&self.path, e))?;
            let mut editor = self.load_in(&tx)?;
            let was = editor.unsaved();
            let out = f(&mut editor)?;
            if editor.unsaved() == was {
                return Ok(out);
            }
            let (saved, _) = self.save_in_tx(&tx, &editor)?;
            tx.commit().map_err(|e| sqlite_err(&self.path, e))?;
            settle(conn);
            (out, saved.compact)
        };
        if compact {
            let _ = self.compact();
        }
        Ok(out)
    }

    pub fn save_if_unchanged(
        &self,
        last_id: &mut Option<FileId>,
        last_dv: &mut Option<i64>,
        editor: &mut Editor,
    ) -> Result<bool> {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let conn = self.live_conn(&mut guard, last_id, last_dv)?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| sqlite_err(&self.path, e))?;
        let dv = query_data_version(&tx, &self.path)?;
        if *last_dv != Some(dv) {
            return Ok(false);
        }
        let (_, commit) = self.save_in_tx(&tx, editor)?;
        tx.commit().map_err(|e| sqlite_err(&self.path, e))?;
        editor.commit_saved(
            self.history_db(),
            (commit.meta.undo, commit.meta.redo),
            commit.meta.generation,
            editor.project().clone(),
            commit.added,
        );
        *last_dv = Some(query_data_version(conn, &self.path)?);
        settle(conn);
        Ok(true)
    }

    pub fn save(&self, editor: &mut Editor) -> Result<Saved> {
        self.with_conn(|conn| {
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|e| sqlite_err(&self.path, e))?;
            let (saved, commit) = self.save_in_tx(&tx, editor)?;
            tx.commit().map_err(|e| sqlite_err(&self.path, e))?;
            settle(conn);
            editor.commit_saved(
                self.history_db(),
                (commit.meta.undo, commit.meta.redo),
                commit.meta.generation,
                editor.project().clone(),
                commit.added,
            );
            Ok(saved)
        })
    }

    fn save_in_tx(&self, tx: &Transaction<'_>, editor: &Editor) -> Result<(Saved, SaveCommit)> {
        sync_project(
            tx,
            self.dir(),
            editor.saved_state(),
            editor.project(),
            Some(&self.blob_hashes),
        )?;
        let meta = HistoryDb::read_meta_in(tx)?;
        let moved = editor.moved();
        let mut added = Vec::new();
        let new_meta = if editor.has_db() {
            if moved.is_empty() {
                meta
            } else {
                if editor.db_generation() != meta.generation {
                    let mut u = meta.undo;
                    let mut r = meta.redo;
                    for m in moved {
                        match m {
                            Moved::Step(_) => {
                                u += 1;
                                r = 0;
                            }
                            Moved::Undo(n) => {
                                if (*n as usize) > u {
                                    return Err(Error::conflict(
                                        "history was compacted past this undo",
                                    ));
                                }
                                u -= *n as usize;
                                r += *n as usize;
                            }
                            Moved::Redo(n) => {
                                let step = (*n as usize).min(r);
                                u += step;
                                r -= step;
                            }
                            Moved::Mark(_) | Moved::Unmark(_) => {}
                        }
                    }
                }
                let (m, inserted) = HistoryDb::append_in(tx, self.dir(), &meta, moved)?;
                added = inserted;
                m
            }
        } else {
            let mut rel_proj = editor.project().clone();
            relativize_project(&mut rel_proj, self.dir());
            let mut rel_h = editor.held().clone();
            relativize_history(&mut rel_h, self.dir());
            let mut base = serde_json::to_value(&rel_proj).map_err(json_err)?;
            for e in rel_h.undo.iter().rev() {
                e.change.apply(&mut base, false).map_err(Error::internal)?;
            }
            HistoryDb::write_whole_in(tx, &base, &rel_h, meta.generation + 1, None, true)?
        };
        clear_dirty(tx, &self.path)?;
        Ok((
            Saved {
                compact: history::due(&new_meta),
            },
            SaveCommit {
                meta: new_meta,
                added,
            },
        ))
    }

    pub fn compact(&self) -> Result<()> {
        self.history_db().compact()
    }
}

fn clear_dirty(conn: &Connection, path: &Path) -> Result<()> {
    conn.execute(
        "UPDATE history_meta SET external_dirty = 0 WHERE id = 1 AND external_dirty != 0",
        [],
    )
    .map_err(|e| sqlite_err(path, e))?;
    Ok(())
}

pub(crate) fn stamp_of(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

fn load_project_from_conn(
    conn: &Connection,
    path: &Path,
    dir: &Path,
    blob_hashes: Option<&BlobHashCache>,
) -> Result<Project> {
    let (rev, edited, meta_json): (u64, Option<u64>, String) = conn
        .query_row("SELECT rev, edited, data FROM meta WHERE id = 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .map_err(|e| Error::invalid(format!("{} is not a vidcut project: {e}", path.display())))?;
    let meta: MetaRow = serde_json::from_str(&meta_json)
        .map_err(|e| Error::invalid(format!("{} is not a vidcut project: {e}", path.display())))?;

    let media: Vec<Media> = {
        let mut stmt = conn
            .prepare_cached("SELECT data FROM media ORDER BY ord ASC")
            .map_err(|e| sqlite_err(path, e))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| sqlite_err(path, e))?;
        let mut out = Vec::new();
        for row in rows {
            let s = row.map_err(|e| sqlite_err(path, e))?;
            out.push(serde_json::from_str(&s).map_err(|e| {
                Error::invalid(format!("{} is not a vidcut project: {e}", path.display()))
            })?);
        }
        out
    };

    let mut frame_cache: HashMap<String, Arc<Vec<[f32; 8]>>> = HashMap::new();
    let mut sample_cache: HashMap<String, Arc<Vec<PathSample>>> = HashMap::new();
    let mut clips_by: HashMap<(String, String), Vec<Clip>> = HashMap::new();
    {
        let mut stmt = conn
            .prepare_cached("SELECT owner, track_id, data FROM clips ORDER BY ord ASC")
            .map_err(|e| sqlite_err(path, e))?;
        let mut rows = stmt.query([]).map_err(|e| sqlite_err(path, e))?;
        while let Some(r) = rows.next().map_err(|e| sqlite_err(path, e))? {
            let owner: String = r.get(0).map_err(|e| sqlite_err(path, e))?;
            let track_id: String = r.get(1).map_err(|e| sqlite_err(path, e))?;
            let raw: String = r.get(2).map_err(|e| sqlite_err(path, e))?;
            let clip = decode_clip(
                conn,
                path,
                &raw,
                &mut frame_cache,
                &mut sample_cache,
                blob_hashes,
            )?;
            clips_by.entry((owner, track_id)).or_default().push(clip);
        }
    }

    let mut tracks_by: HashMap<String, Vec<Track>> = HashMap::new();
    {
        let mut stmt = conn
            .prepare_cached("SELECT owner, data FROM tracks ORDER BY ord ASC")
            .map_err(|e| sqlite_err(path, e))?;
        let mut rows = stmt.query([]).map_err(|e| sqlite_err(path, e))?;
        while let Some(r) = rows.next().map_err(|e| sqlite_err(path, e))? {
            let owner: String = r.get(0).map_err(|e| sqlite_err(path, e))?;
            let raw: String = r.get(1).map_err(|e| sqlite_err(path, e))?;
            let tr: TrackRow = serde_json::from_str(&raw).map_err(|e| {
                Error::invalid(format!("{} is not a vidcut project: {e}", path.display()))
            })?;
            let clips = clips_by
                .remove(&(owner.clone(), tr.id.to_string()))
                .unwrap_or_default();
            tracks_by.entry(owner).or_default().push(Track {
                id: tr.id.into_owned(),
                kind: tr.kind,
                clips: clips.into(),
                locked: tr.locked,
                muted: tr.muted,
                solo: tr.solo,
                volume_db: tr.volume_db,
                voice_isolation: tr.voice_isolation,
                duck: tr.duck.into_owned(),
                equalizer: tr.equalizer.into_owned(),
                dynamics: tr.dynamics,
                pan: tr.pan,
                pan_on: tr.pan_on,
                video_enabled: tr.video_enabled,
                name: tr.name.into_owned(),
                transitions: tr.transitions.into_owned(),
                render_cache: tr.render_cache,
            });
        }
    }

    let mut cues_by: HashMap<(String, String), Vec<Cue>> = HashMap::new();
    {
        let mut stmt = conn
            .prepare_cached("SELECT tl_id, sub_id, data FROM cues ORDER BY ord ASC")
            .map_err(|e| sqlite_err(path, e))?;
        let mut rows = stmt.query([]).map_err(|e| sqlite_err(path, e))?;
        while let Some(r) = rows.next().map_err(|e| sqlite_err(path, e))? {
            let tl_id: String = r.get(0).map_err(|e| sqlite_err(path, e))?;
            let sub_id: String = r.get(1).map_err(|e| sqlite_err(path, e))?;
            let raw: String = r.get(2).map_err(|e| sqlite_err(path, e))?;
            let cue: Cue = serde_json::from_str(&raw).map_err(|e| {
                Error::invalid(format!("{} is not a vidcut project: {e}", path.display()))
            })?;
            cues_by.entry((tl_id, sub_id)).or_default().push(cue);
        }
    }

    let mut subs_by: HashMap<String, Vec<SubtitleTrack>> = HashMap::new();
    {
        let mut stmt = conn
            .prepare_cached("SELECT tl_id, data FROM subtitles ORDER BY ord ASC")
            .map_err(|e| sqlite_err(path, e))?;
        let mut rows = stmt.query([]).map_err(|e| sqlite_err(path, e))?;
        while let Some(r) = rows.next().map_err(|e| sqlite_err(path, e))? {
            let tl_id: String = r.get(0).map_err(|e| sqlite_err(path, e))?;
            let raw: String = r.get(1).map_err(|e| sqlite_err(path, e))?;
            let sr: SubtitleTrackRow = serde_json::from_str(&raw).map_err(|e| {
                Error::invalid(format!("{} is not a vidcut project: {e}", path.display()))
            })?;
            let cues = cues_by
                .remove(&(tl_id.clone(), sr.id.to_string()))
                .unwrap_or_default();
            subs_by.entry(tl_id).or_default().push(SubtitleTrack {
                id: sr.id.into_owned(),
                name: sr.name.into_owned(),
                language: sr.language.into_owned(),
                shown: sr.shown,
                destination: sr.destination,
                locked: sr.locked,
                style: sr.style.into_owned(),
                cues: cues.into(),
            });
        }
    }

    let timelines: Vec<Timeline> = {
        let mut stmt = conn
            .prepare_cached("SELECT data FROM timelines ORDER BY ord ASC")
            .map_err(|e| sqlite_err(path, e))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| sqlite_err(path, e))?;
        let mut out = Vec::new();
        for row in rows {
            let raw = row.map_err(|e| sqlite_err(path, e))?;
            let tr: TimelineRow = serde_json::from_str(&raw).map_err(|e| {
                Error::invalid(format!("{} is not a vidcut project: {e}", path.display()))
            })?;
            let owner = format!("t:{}", tr.id);
            let tracks = tracks_by.remove(&owner).unwrap_or_default();
            let subtitles = subs_by.remove(tr.id.as_ref()).unwrap_or_default();
            out.push(Timeline {
                id: tr.id.into_owned(),
                name: tr.name.into_owned(),
                bin: tr.bin.into_owned(),
                start_tc: tr.start_tc,
                own: tr.own,
                grade: tr.grade.into_owned(),
                render: tr.render,
                clip_filter: tr.clip_filter.into_owned(),
                tracks: tracks.into(),
                marks: tr.marks.into_owned(),
                markers: tr.markers.into_owned(),
                subtitles: subtitles.into(),
            });
        }
        out
    };

    let compounds: Vec<Compound> = {
        let mut stmt = conn
            .prepare_cached("SELECT data FROM compounds ORDER BY ord ASC")
            .map_err(|e| sqlite_err(path, e))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| sqlite_err(path, e))?;
        let mut out = Vec::new();
        for row in rows {
            let raw = row.map_err(|e| sqlite_err(path, e))?;
            let kr: CompoundRow = serde_json::from_str(&raw).map_err(|e| {
                Error::invalid(format!("{} is not a vidcut project: {e}", path.display()))
            })?;
            let owner = format!("c:{}", kr.id);
            let tracks = tracks_by.remove(&owner).unwrap_or_default();
            out.push(Compound {
                id: kr.id.into_owned(),
                name: kr.name.into_owned(),
                bin: kr.bin.into_owned(),
                removed: kr.removed,
                sequence_id: kr.sequence_id.into_owned(),
                fps: kr.fps,
                drop_frame: kr.drop_frame,
                start_tc: kr.start_tc,
                frames: kr.frames,
                tracks: tracks.into(),
                marks: kr.marks.into_owned(),
                markers: kr.markers.into_owned(),
                kept_otio: kr.kept_otio.into_owned(),
            });
        }
        out
    };

    let mut project = Project {
        rev,
        edited,
        settings: meta.settings.into_owned(),
        media: media.into(),
        tracks: SharedVec::new(),
        compounds: compounds.into(),
        compound_path: meta.compound_path.into_owned(),
        marks: Marks::default(),
        tape_marks: meta.tape_marks.into_owned(),
        markers: Vec::new(),
        subtitles: SharedVec::new(),
        timelines: timelines.into(),
        recent: meta.recent.into_owned(),
        next_id: meta.next_id.into_owned(),
        bins: meta.bins.into_owned(),
        groups: meta.groups.into_owned(),
        speakers: meta.speakers.into_owned(),
        queue: meta.queue.into_owned(),
    };
    project.check_out();
    resolve_paths(&mut project, dir);
    let problems = crate::validate::violations(&project);
    if !problems.is_empty() {
        return Err(Error::invalid(format!(
            "{} is inconsistent: {}",
            path.display(),
            problems.join("; ")
        )));
    }
    Ok(project)
}

fn sync_project(
    tx: &Transaction<'_>,
    dir: &Path,
    old: Option<&Project>,
    new: &Project,
    blob_hashes: Option<&BlobHashCache>,
) -> Result<()> {
    let mut row = MetaRow::of(new);
    if old.is_none_or(|old| MetaRow::of(old) != row) {
        relativize_settings(row.settings.to_mut(), dir);
        let data = serde_json::to_string(&row).map_err(json_err)?;
        tx.execute(
            "INSERT OR REPLACE INTO meta (id, rev, edited, data) VALUES (1, ?1, ?2, ?3)",
            params![new.rev, new.edited, data],
        )
        .map_err(|e| Error::internal(e.to_string()))?;
    } else if old.is_some_and(|old| (old.rev, old.edited) != (new.rev, new.edited)) {
        tx.execute(
            "UPDATE meta SET rev = ?1, edited = ?2 WHERE id = 1",
            params![new.rev, new.edited],
        )
        .map_err(|e| Error::internal(e.to_string()))?;
    }

    sync_media(tx, dir, old.map(|p| &p.media), &new.media)?;
    sync_timelines(tx, old, new, blob_hashes)?;
    sync_compounds(tx, old, new, blob_hashes)?;
    Ok(())
}

fn sync_media(
    tx: &Transaction<'_>,
    dir: &Path,
    old: Option<&SharedVec<Media>>,
    new: &SharedVec<Media>,
) -> Result<()> {
    if let Some(old) = old {
        if SharedVec::ptr_eq(old, new) || old == new {
            return Ok(());
        }
        let keep: HashSet<&str> = new.iter().map(|m| m.id.as_str()).collect();
        let mut del = tx
            .prepare_cached("DELETE FROM media WHERE id = ?1")
            .map_err(|e| Error::internal(e.to_string()))?;
        for m in old {
            if !keep.contains(m.id.as_str()) {
                del.execute(params![m.id])
                    .map_err(|e| Error::internal(e.to_string()))?;
            }
        }
        let old_map: HashMap<&str, (usize, &Media)> = old
            .iter()
            .enumerate()
            .map(|(i, m)| (m.id.as_str(), (i, m)))
            .collect();
        let mut ins = tx
            .prepare_cached(
                "INSERT OR REPLACE INTO media (id, ord, path, name, bin, data) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .map_err(|e| Error::internal(e.to_string()))?;
        for (ord, m) in new.iter().enumerate() {
            if old_map.get(m.id.as_str()) == Some(&(ord, m)) {
                continue;
            }
            let mut rel = m.clone();
            relativize_media(&mut rel, dir);
            let data = serde_json::to_string(&rel).map_err(json_err)?;
            ins.execute(params![
                rel.id,
                ord as i64,
                rel.path.to_string_lossy().as_ref(),
                rel.name,
                rel.bin,
                data,
            ])
            .map_err(|e| Error::internal(e.to_string()))?;
        }
        return Ok(());
    }
    tx.execute("DELETE FROM media", [])
        .map_err(|e| Error::internal(e.to_string()))?;
    let mut ins = tx
        .prepare_cached(
            "INSERT INTO media (id, ord, path, name, bin, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .map_err(|e| Error::internal(e.to_string()))?;
    for (ord, m) in new.iter().enumerate() {
        let mut rel = m.clone();
        relativize_media(&mut rel, dir);
        let data = serde_json::to_string(&rel).map_err(json_err)?;
        ins.execute(params![
            rel.id,
            ord as i64,
            rel.path.to_string_lossy().as_ref(),
            rel.name,
            rel.bin,
            data,
        ])
        .map_err(|e| Error::internal(e.to_string()))?;
    }
    Ok(())
}

fn timeline_sheet<'a>(
    p: &'a Project,
    t: &'a Timeline,
) -> (
    &'a SharedVec<Track>,
    &'a Marks,
    &'a Vec<Marker>,
    &'a SharedVec<SubtitleTrack>,
) {
    if p.checked_out_timeline() == Some(t.id.as_str()) {
        (&p.tracks, &p.marks, &p.markers, &p.subtitles)
    } else {
        (&t.tracks, &t.marks, &t.markers, &t.subtitles)
    }
}

fn sync_timelines(
    tx: &Transaction<'_>,
    old: Option<&Project>,
    new: &Project,
    blob_hashes: Option<&BlobHashCache>,
) -> Result<()> {
    if let Some(old) = old {
        let keep: HashSet<&str> = new.timelines.iter().map(|t| t.id.as_str()).collect();
        for t in &old.timelines {
            if !keep.contains(t.id.as_str()) {
                let owner = format!("t:{}", t.id);
                tx.execute("DELETE FROM cues WHERE tl_id = ?1", params![t.id])
                    .map_err(|e| Error::internal(e.to_string()))?;
                tx.execute("DELETE FROM subtitles WHERE tl_id = ?1", params![t.id])
                    .map_err(|e| Error::internal(e.to_string()))?;
                tx.execute("DELETE FROM clips WHERE owner = ?1", params![owner])
                    .map_err(|e| Error::internal(e.to_string()))?;
                tx.execute("DELETE FROM tracks WHERE owner = ?1", params![owner])
                    .map_err(|e| Error::internal(e.to_string()))?;
                tx.execute("DELETE FROM timelines WHERE id = ?1", params![t.id])
                    .map_err(|e| Error::internal(e.to_string()))?;
            }
        }
    } else {
        tx.execute("DELETE FROM cues", [])
            .map_err(|e| Error::internal(e.to_string()))?;
        tx.execute("DELETE FROM subtitles", [])
            .map_err(|e| Error::internal(e.to_string()))?;
        tx.execute("DELETE FROM clips WHERE owner LIKE 't:%'", [])
            .map_err(|e| Error::internal(e.to_string()))?;
        tx.execute("DELETE FROM tracks WHERE owner LIKE 't:%'", [])
            .map_err(|e| Error::internal(e.to_string()))?;
        tx.execute("DELETE FROM timelines", [])
            .map_err(|e| Error::internal(e.to_string()))?;
    }

    let old_map: HashMap<&str, (usize, &Timeline)> = old
        .map(|p| {
            p.timelines
                .iter()
                .enumerate()
                .map(|(i, t)| (t.id.as_str(), (i, t)))
                .collect()
        })
        .unwrap_or_default();

    for (ord, new_t) in new.timelines.iter().enumerate() {
        let (new_tracks, _, _, new_subs) = timeline_sheet(new, new_t);
        let row = TimelineRow::of(new, new_t);
        let old_entry =
            old.zip(old_map.get(new_t.id.as_str()).copied())
                .map(|(old, (old_ord, old_t))| {
                    (
                        old_ord,
                        TimelineRow::of(old, old_t),
                        timeline_sheet(old, old_t),
                    )
                });
        if old_entry
            .as_ref()
            .is_none_or(|(old_ord, old_row, _)| *old_ord != ord || *old_row != row)
        {
            let data = serde_json::to_string(&row).map_err(json_err)?;
            tx.execute(
                "INSERT OR REPLACE INTO timelines (id, ord, name, bin, data) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![new_t.id, ord as i64, new_t.name, new_t.bin, data],
            )
            .map_err(|e| Error::internal(e.to_string()))?;
        }
        let old_sheet = old_entry.map(|(_, _, sheet)| sheet);
        let owner = format!("t:{}", new_t.id);
        sync_tracks(
            tx,
            &owner,
            old_sheet.map(|(t, _, _, _)| t),
            new_tracks,
            blob_hashes,
        )?;
        sync_subtitles(tx, &new_t.id, old_sheet.map(|(_, _, _, s)| s), new_subs)?;
    }
    Ok(())
}

fn compound_sheet<'a>(
    p: &'a Project,
    k: &'a Compound,
) -> (&'a SharedVec<Track>, &'a Marks, &'a Vec<Marker>) {
    if p.checked_out_compound() == Some(k.id.as_str()) {
        (&p.tracks, &p.marks, &p.markers)
    } else {
        (&k.tracks, &k.marks, &k.markers)
    }
}

fn sync_compounds(
    tx: &Transaction<'_>,
    old: Option<&Project>,
    new: &Project,
    blob_hashes: Option<&BlobHashCache>,
) -> Result<()> {
    if let Some(old) = old {
        let keep: HashSet<&str> = new.compounds.iter().map(|k| k.id.as_str()).collect();
        for k in &old.compounds {
            if !keep.contains(k.id.as_str()) {
                let owner = format!("c:{}", k.id);
                tx.execute("DELETE FROM clips WHERE owner = ?1", params![owner])
                    .map_err(|e| Error::internal(e.to_string()))?;
                tx.execute("DELETE FROM tracks WHERE owner = ?1", params![owner])
                    .map_err(|e| Error::internal(e.to_string()))?;
                tx.execute("DELETE FROM compounds WHERE id = ?1", params![k.id])
                    .map_err(|e| Error::internal(e.to_string()))?;
            }
        }
    } else {
        tx.execute("DELETE FROM clips WHERE owner LIKE 'c:%'", [])
            .map_err(|e| Error::internal(e.to_string()))?;
        tx.execute("DELETE FROM tracks WHERE owner LIKE 'c:%'", [])
            .map_err(|e| Error::internal(e.to_string()))?;
        tx.execute("DELETE FROM compounds", [])
            .map_err(|e| Error::internal(e.to_string()))?;
    }

    let old_map: HashMap<&str, (usize, &Compound)> = old
        .map(|p| {
            p.compounds
                .iter()
                .enumerate()
                .map(|(i, k)| (k.id.as_str(), (i, k)))
                .collect()
        })
        .unwrap_or_default();

    for (ord, new_k) in new.compounds.iter().enumerate() {
        let (new_tracks, _, _) = compound_sheet(new, new_k);
        let row = CompoundRow::of(new, new_k);
        let old_entry =
            old.zip(old_map.get(new_k.id.as_str()).copied())
                .map(|(old, (old_ord, old_k))| {
                    (
                        old_ord,
                        CompoundRow::of(old, old_k),
                        compound_sheet(old, old_k).0,
                    )
                });
        if old_entry
            .as_ref()
            .is_none_or(|(old_ord, old_row, _)| *old_ord != ord || *old_row != row)
        {
            let data = serde_json::to_string(&row).map_err(json_err)?;
            tx.execute(
                "INSERT OR REPLACE INTO compounds (id, ord, name, data) VALUES (?1, ?2, ?3, ?4)",
                params![new_k.id, ord as i64, new_k.name, data],
            )
            .map_err(|e| Error::internal(e.to_string()))?;
        }
        let owner = format!("c:{}", new_k.id);
        sync_tracks(
            tx,
            &owner,
            old_entry.map(|(_, _, t)| t),
            new_tracks,
            blob_hashes,
        )?;
    }
    Ok(())
}

fn upsert_track(tx: &Transaction<'_>, owner: &str, ord: usize, tr: &Track) -> Result<()> {
    let row = TrackRow::of(tr);
    let kind = match tr.kind {
        TrackKind::Video => "video",
        TrackKind::Audio => "audio",
    };
    let data = serde_json::to_string(&row).map_err(json_err)?;
    tx.prepare_cached(
        "INSERT OR REPLACE INTO tracks (owner, id, ord, kind, data) VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .map_err(|e| Error::internal(e.to_string()))?
    .execute(params![owner, tr.id, ord as i64, kind, data])
    .map_err(|e| Error::internal(e.to_string()))?;
    Ok(())
}

fn sync_tracks(
    tx: &Transaction<'_>,
    owner: &str,
    old: Option<&SharedVec<Track>>,
    new: &SharedVec<Track>,
    blob_hashes: Option<&BlobHashCache>,
) -> Result<()> {
    if let Some(old) = old {
        if SharedVec::ptr_eq(old, new) {
            return Ok(());
        }
        let keep: HashSet<&str> = new.iter().map(|t| t.id.as_str()).collect();
        for t in old {
            if !keep.contains(t.id.as_str()) {
                tx.execute(
                    "DELETE FROM clips WHERE owner = ?1 AND track_id = ?2",
                    params![owner, t.id],
                )
                .map_err(|e| Error::internal(e.to_string()))?;
                tx.execute(
                    "DELETE FROM tracks WHERE owner = ?1 AND id = ?2",
                    params![owner, t.id],
                )
                .map_err(|e| Error::internal(e.to_string()))?;
            }
        }
        let old_map: HashMap<&str, (usize, &Track)> = old
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id.as_str(), (i, t)))
            .collect();
        for (ord, new_tr) in new.iter().enumerate() {
            let old_entry = old_map.get(new_tr.id.as_str()).copied();
            if old_entry.is_none_or(|(old_ord, old_tr)| {
                old_ord != ord || TrackRow::of(old_tr) != TrackRow::of(new_tr)
            }) {
                upsert_track(tx, owner, ord, new_tr)?;
            }
            sync_clips(
                tx,
                owner,
                &new_tr.id,
                old_entry.map(|(_, t)| &t.clips),
                &new_tr.clips,
                blob_hashes,
            )?;
        }
        return Ok(());
    }
    for (ord, new_tr) in new.iter().enumerate() {
        upsert_track(tx, owner, ord, new_tr)?;
        sync_clips(tx, owner, &new_tr.id, None, &new_tr.clips, blob_hashes)?;
    }
    Ok(())
}

fn sync_clips(
    tx: &Transaction<'_>,
    owner: &str,
    track_id: &str,
    old: Option<&SharedVec<Clip>>,
    new: &SharedVec<Clip>,
    blob_hashes: Option<&BlobHashCache>,
) -> Result<()> {
    if let Some(old) = old {
        if SharedVec::ptr_eq(old, new) {
            return Ok(());
        }
        let keep: HashSet<&str> = new.iter().map(|c| c.id.as_str()).collect();
        {
            let mut del = tx
                .prepare_cached("DELETE FROM clips WHERE owner = ?1 AND track_id = ?2 AND id = ?3")
                .map_err(|e| Error::internal(e.to_string()))?;
            for c in old {
                if !keep.contains(c.id.as_str()) {
                    del.execute(params![owner, track_id, c.id])
                        .map_err(|e| Error::internal(e.to_string()))?;
                }
            }
        }
        let old_map: HashMap<&str, (usize, &Clip)> = old
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id.as_str(), (i, c)))
            .collect();
        for (ord, new_c) in new.iter().enumerate() {
            match old_map.get(new_c.id.as_str()).copied() {
                Some((old_ord, old_c)) if old_ord == ord && old_c == new_c => {}
                Some((_, old_c)) if old_c == new_c => {
                    tx.prepare_cached(
                        "UPDATE clips SET ord = ?1 WHERE owner = ?2 AND track_id = ?3 AND id = ?4",
                    )
                    .map_err(|e| Error::internal(e.to_string()))?
                    .execute(params![ord as i64, owner, track_id, new_c.id])
                    .map_err(|e| Error::internal(e.to_string()))?;
                }
                _ => {
                    let data = encode_clip(tx, new_c, blob_hashes)?;
                    tx.prepare_cached(
                        "INSERT OR REPLACE INTO clips \
                         (owner, track_id, id, ord, start, duration, media_id, data) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    )
                    .map_err(|e| Error::internal(e.to_string()))?
                    .execute(params![
                        owner,
                        track_id,
                        new_c.id,
                        ord as i64,
                        new_c.start,
                        new_c.duration,
                        new_c.media,
                        data,
                    ])
                    .map_err(|e| Error::internal(e.to_string()))?;
                }
            }
        }
        return Ok(());
    }
    for (ord, new_c) in new.iter().enumerate() {
        let data = encode_clip(tx, new_c, blob_hashes)?;
        tx.prepare_cached(
            "INSERT INTO clips (owner, track_id, id, ord, start, duration, media_id, data) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .map_err(|e| Error::internal(e.to_string()))?
        .execute(params![
            owner,
            track_id,
            new_c.id,
            ord as i64,
            new_c.start,
            new_c.duration,
            new_c.media,
            data,
        ])
        .map_err(|e| Error::internal(e.to_string()))?;
    }
    Ok(())
}

fn upsert_subtitle_track(
    tx: &Transaction<'_>,
    tl_id: &str,
    ord: usize,
    sr: &SubtitleTrack,
) -> Result<()> {
    let row = SubtitleTrackRow::of(sr);
    let data = serde_json::to_string(&row).map_err(json_err)?;
    tx.prepare_cached(
        "INSERT OR REPLACE INTO subtitles (tl_id, id, ord, data) VALUES (?1, ?2, ?3, ?4)",
    )
    .map_err(|e| Error::internal(e.to_string()))?
    .execute(params![tl_id, sr.id, ord as i64, data])
    .map_err(|e| Error::internal(e.to_string()))?;
    Ok(())
}

fn sync_subtitles(
    tx: &Transaction<'_>,
    tl_id: &str,
    old: Option<&SharedVec<SubtitleTrack>>,
    new: &SharedVec<SubtitleTrack>,
) -> Result<()> {
    if let Some(old) = old {
        if SharedVec::ptr_eq(old, new) {
            return Ok(());
        }
        let keep: HashSet<&str> = new.iter().map(|s| s.id.as_str()).collect();
        for s in old {
            if !keep.contains(s.id.as_str()) {
                tx.execute(
                    "DELETE FROM cues WHERE tl_id = ?1 AND sub_id = ?2",
                    params![tl_id, s.id],
                )
                .map_err(|e| Error::internal(e.to_string()))?;
                tx.execute(
                    "DELETE FROM subtitles WHERE tl_id = ?1 AND id = ?2",
                    params![tl_id, s.id],
                )
                .map_err(|e| Error::internal(e.to_string()))?;
            }
        }
        let old_map: HashMap<&str, (usize, &SubtitleTrack)> = old
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id.as_str(), (i, s)))
            .collect();
        for (ord, new_s) in new.iter().enumerate() {
            let old_entry = old_map.get(new_s.id.as_str()).copied();
            if old_entry.is_none_or(|(old_ord, old_s)| {
                old_ord != ord || SubtitleTrackRow::of(old_s) != SubtitleTrackRow::of(new_s)
            }) {
                upsert_subtitle_track(tx, tl_id, ord, new_s)?;
            }
            sync_cues(
                tx,
                tl_id,
                &new_s.id,
                old_entry.map(|(_, s)| &s.cues),
                &new_s.cues,
            )?;
        }
        return Ok(());
    }
    for (ord, new_s) in new.iter().enumerate() {
        upsert_subtitle_track(tx, tl_id, ord, new_s)?;
        sync_cues(tx, tl_id, &new_s.id, None, &new_s.cues)?;
    }
    Ok(())
}

fn sync_cues(
    tx: &Transaction<'_>,
    tl_id: &str,
    sub_id: &str,
    old: Option<&SharedVec<Cue>>,
    new: &SharedVec<Cue>,
) -> Result<()> {
    if let Some(old) = old {
        if SharedVec::ptr_eq(old, new) {
            return Ok(());
        }
        let keep: HashSet<&str> = new.iter().map(|c| c.id.as_str()).collect();
        {
            let mut del = tx
                .prepare_cached("DELETE FROM cues WHERE tl_id = ?1 AND sub_id = ?2 AND id = ?3")
                .map_err(|e| Error::internal(e.to_string()))?;
            for c in old {
                if !keep.contains(c.id.as_str()) {
                    del.execute(params![tl_id, sub_id, c.id])
                        .map_err(|e| Error::internal(e.to_string()))?;
                }
            }
        }
        let old_map: HashMap<&str, (usize, &Cue)> = old
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id.as_str(), (i, c)))
            .collect();
        for (ord, new_c) in new.iter().enumerate() {
            match old_map.get(new_c.id.as_str()).copied() {
                Some((old_ord, old_c)) if old_ord == ord && old_c == new_c => {}
                Some((_, old_c)) if old_c == new_c => {
                    tx.prepare_cached(
                        "UPDATE cues SET ord = ?1 WHERE tl_id = ?2 AND sub_id = ?3 AND id = ?4",
                    )
                    .map_err(|e| Error::internal(e.to_string()))?
                    .execute(params![ord as i64, tl_id, sub_id, new_c.id])
                    .map_err(|e| Error::internal(e.to_string()))?;
                }
                _ => {
                    let data = serde_json::to_string(new_c).map_err(json_err)?;
                    tx.prepare_cached(
                        "INSERT OR REPLACE INTO cues \
                         (tl_id, sub_id, id, ord, start, duration, data) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    )
                    .map_err(|e| Error::internal(e.to_string()))?
                    .execute(params![
                        tl_id,
                        sub_id,
                        new_c.id,
                        ord as i64,
                        new_c.start,
                        new_c.duration,
                        data,
                    ])
                    .map_err(|e| Error::internal(e.to_string()))?;
                }
            }
        }
        return Ok(());
    }
    let mut ins = tx
        .prepare_cached(
            "INSERT INTO cues (tl_id, sub_id, id, ord, start, duration, data) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .map_err(|e| Error::internal(e.to_string()))?;
    for (ord, new_c) in new.iter().enumerate() {
        let data = serde_json::to_string(new_c).map_err(json_err)?;
        ins.execute(params![
            tl_id,
            sub_id,
            new_c.id,
            ord as i64,
            new_c.start,
            new_c.duration,
            data,
        ])
        .map_err(|e| Error::internal(e.to_string()))?;
    }
    Ok(())
}

fn clip_has_heavy_blobs(clip: &Clip) -> bool {
    let Some(pic) = clip.picture.as_deref() else {
        return false;
    };
    if pic
        .stabilization
        .as_ref()
        .and_then(|s| s.made.as_ref())
        .is_some_and(|m| m.frames.len() >= BLOB_MIN_SAMPLES)
    {
        return true;
    }
    pic.composition.as_ref().is_some_and(|comp| {
        comp.nodes.iter().any(|n| match &n.tool {
            Tool::Tracker(t) => t.patterns.iter().any(|p| {
                p.path
                    .as_ref()
                    .is_some_and(|mp| mp.samples.len() >= BLOB_MIN_SAMPLES)
            }),
            _ => false,
        })
    })
}

fn cached_blob_hash(
    tx: &Transaction<'_>,
    blob_hashes: Option<&BlobHashCache>,
    ptr: usize,
    matches: impl FnOnce(&BlobWeak) -> bool,
) -> Result<Option<String>> {
    let hit = blob_hashes.and_then(|cache| {
        let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        let (weak, hash) = guard.get(&ptr)?;
        matches(weak).then(|| hash.clone())
    });
    let Some(hash) = hit else {
        return Ok(None);
    };
    let present: i64 = tx
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM blobs WHERE hash = ?1)")
        .map_err(|e| Error::internal(e.to_string()))?
        .query_row(params![&hash], |r| r.get(0))
        .map_err(|e| Error::internal(e.to_string()))?;
    Ok((present != 0).then_some(hash))
}

fn write_blob(tx: &Transaction<'_>, raw: &[u8]) -> Result<String> {
    let hash = crate::fingerprint::digest(raw);
    let compressed = compress_bytes(raw)?;
    tx.prepare_cached("INSERT OR IGNORE INTO blobs (hash, data) VALUES (?1, ?2)")
        .map_err(|e| Error::internal(e.to_string()))?
        .execute(params![&hash, compressed])
        .map_err(|e| Error::internal(e.to_string()))?;
    Ok(hash)
}

fn store_arc_blob<T: Serialize>(
    tx: &Transaction<'_>,
    arc: &Arc<Vec<T>>,
    blob_hashes: Option<&BlobHashCache>,
    matches: impl FnOnce(&BlobWeak) -> bool,
    wrap: impl FnOnce(Weak<Vec<T>>) -> BlobWeak,
) -> Result<String> {
    let ptr = Arc::as_ptr(arc) as usize;
    if let Some(hash) = cached_blob_hash(tx, blob_hashes, ptr, matches)? {
        return Ok(hash);
    }
    let raw = serde_json::to_vec(&**arc).map_err(json_err)?;
    let hash = write_blob(tx, &raw)?;
    if let Some(cache) = blob_hashes {
        let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        guard.insert(ptr, (wrap(Arc::downgrade(arc)), hash.clone()));
    }
    Ok(hash)
}

fn store_samples_blob(
    tx: &Transaction<'_>,
    samples: &Arc<Vec<PathSample>>,
    blob_hashes: Option<&BlobHashCache>,
) -> Result<String> {
    store_arc_blob(
        tx,
        samples,
        blob_hashes,
        |w| matches!(w, BlobWeak::Samples(weak) if weak.upgrade().is_some_and(|a| Arc::ptr_eq(&a, samples))),
        BlobWeak::Samples,
    )
}

fn store_frames_blob(
    tx: &Transaction<'_>,
    frames: &Arc<Vec<[f32; 8]>>,
    blob_hashes: Option<&BlobHashCache>,
) -> Result<String> {
    store_arc_blob(
        tx,
        frames,
        blob_hashes,
        |w| matches!(w, BlobWeak::Frames(weak) if weak.upgrade().is_some_and(|a| Arc::ptr_eq(&a, frames))),
        BlobWeak::Frames,
    )
}

fn encode_clip(
    tx: &Transaction<'_>,
    clip: &Clip,
    blob_hashes: Option<&BlobHashCache>,
) -> Result<String> {
    if !clip_has_heavy_blobs(clip) {
        return serde_json::to_string(clip).map_err(json_err);
    }
    let mut light = clip.clone();
    let mut stab_hash = None;
    let mut sample_hashes = Vec::new();
    if let Some(pic) = light.picture.as_deref_mut() {
        if let Some(made) = pic
            .stabilization
            .as_mut()
            .and_then(|s| s.made.as_mut())
            .filter(|m| m.frames.len() >= BLOB_MIN_SAMPLES)
        {
            stab_hash = Some(store_frames_blob(tx, &made.frames, blob_hashes)?);
            made.frames = Arc::new(Vec::new());
        }
        if let Some(comp) = pic.composition.as_mut() {
            for (ni, node) in comp.nodes.iter_mut().enumerate() {
                let Tool::Tracker(tracker) = &mut node.tool else {
                    continue;
                };
                for (pi, pat) in tracker.patterns.iter_mut().enumerate() {
                    if let Some(mp) = pat
                        .path
                        .as_mut()
                        .filter(|mp| mp.samples.len() >= BLOB_MIN_SAMPLES)
                    {
                        let hash = store_samples_blob(tx, &mp.samples, blob_hashes)?;
                        sample_hashes.push((ni, pi, hash));
                        mp.samples = Arc::new(Vec::new());
                    }
                }
            }
        }
    }
    let mut val = serde_json::to_value(&light).map_err(json_err)?;
    if let Some(pic) = val.get_mut("picture").and_then(Value::as_object_mut) {
        if let Some(hash) = stab_hash
            && let Some(frames) = pic
                .get_mut("stabilization")
                .and_then(Value::as_object_mut)
                .and_then(|s| s.get_mut("made"))
                .and_then(Value::as_object_mut)
                .and_then(|m| m.get_mut("frames"))
        {
            *frames = json!({ "$blob": hash });
        }
        if !sample_hashes.is_empty()
            && let Some(nodes) = pic
                .get_mut("composition")
                .and_then(Value::as_object_mut)
                .and_then(|c| c.get_mut("nodes"))
                .and_then(Value::as_array_mut)
        {
            for (ni, pi, hash) in sample_hashes {
                if let Some(samples) = nodes
                    .get_mut(ni)
                    .and_then(|n| n.get_mut("patterns"))
                    .and_then(Value::as_array_mut)
                    .and_then(|pats| pats.get_mut(pi))
                    .and_then(|p| p.get_mut("path"))
                    .and_then(Value::as_object_mut)
                    .and_then(|mp| mp.get_mut("samples"))
                {
                    *samples = json!({ "$blob": hash });
                }
            }
        }
    }
    serde_json::to_string(&val).map_err(json_err)
}

fn take_blob_ref(slot: &mut Value) -> Option<String> {
    let hash = slot
        .as_object()
        .filter(|map| map.len() == 1)
        .and_then(|map| map.get("$blob"))
        .and_then(Value::as_str)
        .map(String::from)?;
    *slot = Value::Array(Vec::new());
    Some(hash)
}

fn load_arc_blob<T: DeserializeOwned>(
    conn: &Connection,
    path: &Path,
    hash: &str,
    cache: &mut HashMap<String, Arc<Vec<T>>>,
    blob_hashes: Option<&BlobHashCache>,
    wrap: impl FnOnce(Weak<Vec<T>>) -> BlobWeak,
) -> Result<Arc<Vec<T>>> {
    if let Some(hit) = cache.get(hash) {
        return Ok(Arc::clone(hit));
    }
    let compressed: Vec<u8> = conn
        .prepare_cached("SELECT data FROM blobs WHERE hash = ?1")
        .map_err(|e| sqlite_err(path, e))?
        .query_row(params![hash], |r| r.get(0))
        .map_err(|e| Error::invalid(format!("{} missing blob {hash}: {e}", path.display())))?;
    let raw = decompress_bytes(&compressed)?;
    let items: Vec<T> = serde_json::from_slice(&raw)
        .map_err(|e| Error::invalid(format!("{} is not a vidcut project: {e}", path.display())))?;
    let arc = Arc::new(items);
    cache.insert(hash.to_string(), Arc::clone(&arc));
    if let Some(hashes) = blob_hashes {
        let mut guard = hashes.lock().unwrap_or_else(|e| e.into_inner());
        guard.insert(
            Arc::as_ptr(&arc) as usize,
            (wrap(Arc::downgrade(&arc)), hash.to_string()),
        );
    }
    Ok(arc)
}

fn decode_clip(
    conn: &Connection,
    path: &Path,
    raw: &str,
    frame_cache: &mut HashMap<String, Arc<Vec<[f32; 8]>>>,
    sample_cache: &mut HashMap<String, Arc<Vec<PathSample>>>,
    blob_hashes: Option<&BlobHashCache>,
) -> Result<Clip> {
    if !raw.contains("\"$blob\"") {
        return serde_json::from_str(raw).map_err(|e| {
            Error::invalid(format!("{} is not a vidcut project: {e}", path.display()))
        });
    }
    let mut val: Value = serde_json::from_str(raw)
        .map_err(|e| Error::invalid(format!("{} is not a vidcut project: {e}", path.display())))?;
    let mut stab_hash = None;
    let mut sample_hashes = Vec::new();
    if let Some(pic) = val.get_mut("picture").and_then(Value::as_object_mut) {
        if let Some(frames) = pic
            .get_mut("stabilization")
            .and_then(Value::as_object_mut)
            .and_then(|s| s.get_mut("made"))
            .and_then(Value::as_object_mut)
            .and_then(|m| m.get_mut("frames"))
            && let Some(hash) = take_blob_ref(frames)
        {
            stab_hash = Some(hash);
        }
        if let Some(nodes) = pic
            .get_mut("composition")
            .and_then(Value::as_object_mut)
            .and_then(|c| c.get_mut("nodes"))
            .and_then(Value::as_array_mut)
        {
            for (ni, node) in nodes.iter_mut().enumerate() {
                let Some(patterns) = node.get_mut("patterns").and_then(Value::as_array_mut) else {
                    continue;
                };
                for (pi, pat) in patterns.iter_mut().enumerate() {
                    if let Some(samples) = pat
                        .get_mut("path")
                        .and_then(Value::as_object_mut)
                        .and_then(|p| p.get_mut("samples"))
                        && let Some(hash) = take_blob_ref(samples)
                    {
                        sample_hashes.push((ni, pi, hash));
                    }
                }
            }
        }
    }
    let mut clip: Clip = serde_json::from_value(val)
        .map_err(|e| Error::invalid(format!("{} is not a vidcut project: {e}", path.display())))?;
    if let Some(pic) = clip.picture.as_deref_mut() {
        if let Some(hash) = stab_hash {
            let arc = load_arc_blob(
                conn,
                path,
                &hash,
                frame_cache,
                blob_hashes,
                BlobWeak::Frames,
            )?;
            if let Some(made) = pic.stabilization.as_mut().and_then(|s| s.made.as_mut()) {
                made.frames = arc;
            }
        }
        if !sample_hashes.is_empty()
            && let Some(comp) = pic.composition.as_mut()
        {
            for (ni, pi, hash) in sample_hashes {
                let arc = load_arc_blob(
                    conn,
                    path,
                    &hash,
                    sample_cache,
                    blob_hashes,
                    BlobWeak::Samples,
                )?;
                if let Some(node) = comp.nodes.get_mut(ni)
                    && let Tool::Tracker(tracker) = &mut node.tool
                    && let Some(pat) = tracker.patterns.get_mut(pi)
                    && let Some(mp) = pat.path.as_mut()
                {
                    mp.samples = arc;
                }
            }
        }
    }
    Ok(clip)
}

fn collect_blob_refs(val: &Value, used: &mut HashSet<String>) {
    match val {
        Value::Array(items) => {
            for item in items {
                collect_blob_refs(item, used);
            }
        }
        Value::Object(map) => {
            if map.len() == 1
                && let Some(hash) = map.get("$blob").and_then(Value::as_str)
            {
                used.insert(hash.to_string());
                return;
            }
            for child in map.values() {
                collect_blob_refs(child, used);
            }
        }
        _ => {}
    }
}

pub(crate) fn gc_blobs(tx: &Transaction<'_>) -> Result<()> {
    let mut used = HashSet::new();
    {
        let mut stmt = tx
            .prepare_cached("SELECT data FROM clips WHERE instr(data, '\"$blob\"') > 0")
            .map_err(|e| Error::internal(e.to_string()))?;
        let mut rows = stmt.query([]).map_err(|e| Error::internal(e.to_string()))?;
        while let Some(r) = rows.next().map_err(|e| Error::internal(e.to_string()))? {
            let raw: String = r.get(0).map_err(|e| Error::internal(e.to_string()))?;
            if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                collect_blob_refs(&val, &mut used);
            }
        }
    }
    let hashes: Vec<String> = {
        let mut stmt = tx
            .prepare_cached("SELECT hash FROM blobs")
            .map_err(|e| Error::internal(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| Error::internal(e.to_string()))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| Error::internal(e.to_string()))?
    };
    let mut del = tx
        .prepare_cached("DELETE FROM blobs WHERE hash = ?1")
        .map_err(|e| Error::internal(e.to_string()))?;
    for h in hashes {
        if !used.contains(&h) {
            del.execute(params![h])
                .map_err(|e| Error::internal(e.to_string()))?;
        }
    }
    Ok(())
}

fn relativize_path(path: &mut PathBuf, dir: &Path) {
    if let Ok(rel) = path.strip_prefix(dir) {
        *path = portable(rel);
    }
}

fn relativize_path_str(s: &mut String, dir: &Path) {
    let mut path = PathBuf::from(&*s);
    relativize_path(&mut path, dir);
    *s = path.to_string_lossy().into_owned();
}

fn resolve_path(file: &mut PathBuf, dir: &Path) {
    if !absolute_anywhere(file) {
        *file = dir.join(native(file));
    }
}

fn resolve_path_str(s: &mut String, dir: &Path) {
    let mut path = PathBuf::from(&*s);
    resolve_path(&mut path, dir);
    *s = path.to_string_lossy().into_owned();
}

fn map_media_value(v: &mut Value, map: &impl Fn(&mut String)) {
    let Some(obj) = v.as_object_mut() else {
        return;
    };
    for key in ["path", "input_lut"] {
        if let Some(Value::String(s)) = obj.get_mut(key) {
            map(s);
        }
    }
}

fn map_lut_value(v: &mut Value, map: &impl Fn(&mut String)) {
    if let Some(obj) = v.as_object_mut()
        && let Some(Value::String(s)) = obj.get_mut("file")
    {
        map(s);
    }
}

fn map_settings_value(v: &mut Value, map: &impl Fn(&mut String)) {
    let Some(obj) = v.as_object_mut() else {
        return;
    };
    for key in ["input_lut", "output_lut", "video_monitor_lut"] {
        if let Some(Value::String(s)) = obj.get_mut(key) {
            map(s);
        }
    }
    for key in ["color_viewer_lut", "scopes_lut"] {
        if let Some(lut) = obj.get_mut(key) {
            map_lut_value(lut, map);
        }
    }
}

fn map_set_str(c: &mut Change, map: &impl Fn(&mut String)) {
    if let Change::Set { before, after } = c {
        for slot in [before.as_mut(), after.as_mut()].into_iter().flatten() {
            if let Value::String(s) = slot {
                map(s);
            }
        }
    }
}

fn map_lut_change(c: &mut Change, map: &impl Fn(&mut String)) {
    match c {
        Change::Set { before, after } => {
            for slot in [before.as_mut(), after.as_mut()].into_iter().flatten() {
                map_lut_value(slot, map);
            }
        }
        Change::Fields(fields) => {
            if let Some(file) = fields.get_mut("file") {
                map_set_str(file, map);
            }
        }
        Change::Items(_) => {}
    }
}

fn map_settings_change(c: &mut Change, map: &impl Fn(&mut String)) {
    match c {
        Change::Set { before, after } => {
            for slot in [before.as_mut(), after.as_mut()].into_iter().flatten() {
                map_settings_value(slot, map);
            }
        }
        Change::Fields(fields) => {
            for key in ["input_lut", "output_lut", "video_monitor_lut"] {
                if let Some(field) = fields.get_mut(key) {
                    map_set_str(field, map);
                }
            }
            for key in ["color_viewer_lut", "scopes_lut"] {
                if let Some(lut) = fields.get_mut(key) {
                    map_lut_change(lut, map);
                }
            }
        }
        Change::Items(_) => {}
    }
}

fn map_media_change(c: &mut Change, map: &impl Fn(&mut String)) {
    match c {
        Change::Set { before, after } => {
            for slot in [before.as_mut(), after.as_mut()].into_iter().flatten() {
                map_media_value(slot, map);
            }
        }
        Change::Fields(fields) => {
            for key in ["path", "input_lut"] {
                if let Some(field) = fields.get_mut(key) {
                    map_set_str(field, map);
                }
            }
        }
        Change::Items(_) => {}
    }
}

fn map_media_list_change(c: &mut Change, map: &impl Fn(&mut String)) {
    match c {
        Change::Set { before, after } => {
            for slot in [before.as_mut(), after.as_mut()].into_iter().flatten() {
                if let Some(items) = slot.as_array_mut() {
                    for item in items {
                        map_media_value(item, map);
                    }
                }
            }
        }
        Change::Items(items) => {
            items.map_items(|v| map_media_value(v, map), |c| map_media_change(c, map));
        }
        Change::Fields(_) => {}
    }
}

fn map_change(change: &mut Change, map: &impl Fn(&mut String)) {
    match change {
        Change::Set { before, after } => {
            for slot in [before.as_mut(), after.as_mut()].into_iter().flatten() {
                let Some(obj) = slot.as_object_mut() else {
                    continue;
                };
                if let Some(Value::Array(items)) = obj.get_mut("media") {
                    for item in items {
                        map_media_value(item, map);
                    }
                }
                if let Some(settings) = obj.get_mut("settings") {
                    map_settings_value(settings, map);
                }
            }
        }
        Change::Fields(fields) => {
            if let Some(media) = fields.get_mut("media") {
                map_media_list_change(media, map);
            }
            if let Some(settings) = fields.get_mut("settings") {
                map_settings_change(settings, map);
            }
        }
        Change::Items(_) => {}
    }
}

pub(crate) fn relativize_project(project: &mut Project, dir: &Path) {
    relativize_settings(&mut project.settings, dir);
    for m in &mut project.media {
        relativize_media(m, dir);
    }
}

pub(crate) fn relativize_change(change: &mut Change, dir: &Path) {
    map_change(change, &|s| relativize_path_str(s, dir));
}

pub(crate) fn resolve_change(change: &mut Change, dir: &Path) {
    map_change(change, &|s| resolve_path_str(s, dir));
}

pub(crate) fn relativize_history(history: &mut History, dir: &Path) {
    fn map_branch(branches: &mut [crate::steps::Branch<Entry>], dir: &Path) {
        for b in branches {
            for e in &mut b.steps {
                relativize_change(&mut e.change, dir);
            }
            map_branch(&mut b.branches, dir);
        }
    }
    for e in history.undo.iter_mut().chain(&mut history.redo) {
        relativize_change(&mut e.change, dir);
    }
    map_branch(&mut history.branches, dir);
}

fn relativize_media(m: &mut Media, dir: &Path) {
    relativize_path(&mut m.path, dir);
    if let Some(lut) = m.input_lut.as_mut() {
        relativize_path(lut, dir);
    }
}

fn relativize_settings(settings: &mut Settings, dir: &Path) {
    if let Some(p) = settings.input_lut.as_mut() {
        relativize_path(p, dir);
    }
    if let Some(p) = settings.output_lut.as_mut() {
        relativize_path(p, dir);
    }
    if let Some(p) = settings.video_monitor_lut.as_mut() {
        relativize_path(p, dir);
    }
    for lut in [&mut settings.color_viewer_lut, &mut settings.scopes_lut] {
        if let crate::model::MonitorLut::File(path) = lut {
            relativize_path(path, dir);
        }
    }
}

pub(crate) fn resolve_paths(project: &mut Project, dir: &Path) {
    let Project {
        media, settings, ..
    } = project;
    for m in media {
        resolve_path(&mut m.path, dir);
        if let Some(lut) = m.input_lut.as_mut() {
            resolve_path(lut, dir);
        }
    }
    if let Some(p) = settings.input_lut.as_mut() {
        resolve_path(p, dir);
    }
    if let Some(p) = settings.output_lut.as_mut() {
        resolve_path(p, dir);
    }
    if let Some(p) = settings.video_monitor_lut.as_mut() {
        resolve_path(p, dir);
    }
    for lut in [&mut settings.color_viewer_lut, &mut settings.scopes_lut] {
        if let crate::model::MonitorLut::File(path) = lut {
            resolve_path(path, dir);
        }
    }
}

fn portable(relative: &Path) -> PathBuf {
    #[cfg(windows)]
    let path = PathBuf::from(relative.to_string_lossy().replace('\\', "/"));
    #[cfg(not(windows))]
    let path = relative.to_path_buf();
    path
}

fn native(relative: &Path) -> PathBuf {
    #[cfg(windows)]
    let path = PathBuf::from(relative.to_string_lossy().replace('/', "\\"));
    #[cfg(not(windows))]
    let path = relative.to_path_buf();
    path
}

fn absolute_anywhere(path: &Path) -> bool {
    let Some(s) = path.to_str() else {
        return path.is_absolute();
    };
    s.starts_with(['/', '\\']) || windows_absolute(s)
}

pub fn from_another_system(path: &Path) -> bool {
    path.to_str().is_some_and(absolute_elsewhere)
}

#[cfg(windows)]
pub(crate) fn absolute_elsewhere(s: &str) -> bool {
    s.starts_with('/') && !s.starts_with("//")
}

#[cfg(not(windows))]
pub(crate) fn absolute_elsewhere(s: &str) -> bool {
    windows_absolute(s)
}

pub(crate) fn windows_absolute(s: &str) -> bool {
    s.starts_with("\\\\")
        || (s.get(..2).is_some_and(is_drive) && matches!(s.as_bytes().get(2), Some(b'\\' | b'/')))
}

pub(crate) fn is_drive(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::model::{
        ClipCache, ClipRetime, Composition, MotionPath, Node, PathSample, Pattern, Picture,
        Settings, Stabilization, Stabilized, Stabilizing, Tracker,
    };
    use crate::request::Envelope;
    use crate::test_support::FakeEnv;

    fn project_in(dir: &Path) -> Store {
        let path = dir.join("p.vidcut");
        let project = Project::new(Settings {
            auto_format: false,
            ..Settings::default()
        });
        Store::create(&path, &project).unwrap()
    }

    fn add_bin(store: &Store, name: &str) {
        let request = json!({"method": "bins.add", "params": {"path": name}});
        store
            .transact(|ed| {
                ed.apply(
                    Envelope::from_json(request).unwrap(),
                    &FakeEnv::default(),
                    None,
                )
            })
            .unwrap();
    }

    fn external_flip_ripple(path: &Path) {
        let conn = open_conn(path).unwrap();
        let data: String = conn
            .query_row("SELECT data FROM meta WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        let swapped = data.replace("\"ripple_on\":true", "\"ripple_on\":false");
        conn.execute("UPDATE meta SET data = ?1 WHERE id = 1", params![swapped])
            .unwrap();
    }

    fn motion_clip(id: &str, start: i64, samples: Vec<PathSample>) -> Clip {
        Clip {
            id: id.into(),
            media: "m1".into(),
            start,
            duration: 20,
            source_in: 0,
            phase: 0.0,
            video: true,
            audio: None,
            enabled: true,
            name: None,
            effects: None,
            generator: None,
            title: None,
            adjustment: false,
            picture: Some(
                Picture {
                    composition: Some(Composition {
                        nodes: vec![Node {
                            name: "Tracker1".into(),
                            tool: Tool::Tracker(Tracker {
                                patterns: vec![Pattern {
                                    name: "Pattern 1".into(),
                                    enabled: true,
                                    path: Some(MotionPath {
                                        subject: None,
                                        tracked: true,
                                        samples: Arc::new(samples),
                                    }),
                                    center: None,
                                    width: None,
                                    height: None,
                                    offset: None,
                                    kind: None,
                                    search_width: None,
                                    search_height: None,
                                    channel: None,
                                }],
                                operation: None,
                                merge: None,
                                edges: None,
                                apply_mode: None,
                                operator: None,
                                subtractive_additive: None,
                                match_position: true,
                                match_rotation: true,
                                match_scaling: true,
                                pivot_type: None,
                                pivot_tracker: None,
                                pivot: None,
                                reference_time: None,
                                custom_time: None,
                                intermediate_points: None,
                                fade_frames: None,
                                adaptive_mode: None,
                                match_tolerance: None,
                            }),
                            input: None,
                            foreground: None,
                            effect_mask: None,
                            detail_map: None,
                            brightness_map: None,
                            pass_through: false,
                            locked: false,
                            blend: None,
                            blur_alpha: true,
                            multiply_by_mask: false,
                            mask_channel: None,
                            mask_range: None,
                            mask_clip: None,
                            place: [0.0, 0.0],
                            keys: Vec::new(),
                            connect: None,
                        }],
                        loop_play: true,
                        ping_pong: false,
                        start: 0,
                    }),
                    ..Picture::default()
                }
                .into(),
            ),
            compound: None,
            speed: None,
            retime: ClipRetime::Project,
            color: None,
            render_cache: ClipCache::Auto,
            link: None,
        }
    }

    fn sample_path() -> Vec<PathSample> {
        (0..20)
            .map(|frame| PathSample {
                frame,
                center: [0.5, 0.5],
                size: [0.1, 0.1],
                rotation: 0.0,
            })
            .collect()
    }

    #[test]
    fn a_saved_project_keeps_its_paths_absolute() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = crate::dirs::canonical(tmp.path()).unwrap();
        let fixture = crate::test_support::Fixture::new(&[(4.0, true)]);
        let mut project = fixture.editor.project().clone();
        project.media[0].path = dir.join("1.mp4");
        let path = dir.join("p.vidcut");
        let store = Store::create(&path, &project).unwrap();
        let mut editor = store.load().unwrap();
        store.save(&mut editor).unwrap();
        assert_eq!(editor.project().media[0].path, dir.join("1.mp4"));
    }

    #[test]
    fn a_save_adds_events_to_the_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = project_in(dir.path());
        add_bin(&store, "a");
        let before = store.history_db().meta().unwrap().steps;
        add_bin(&store, "b");
        let after = store.history_db().meta().unwrap().steps;
        assert_eq!((before, after), (1, 2));
    }

    #[test]
    fn an_outside_edit_is_one_undo_step() {
        let dir = tempfile::tempdir().unwrap();
        let store = project_in(dir.path());
        add_bin(&store, "a");
        external_flip_ripple(store.path());
        let newest = store.load().unwrap().history_view(1).unwrap().undo;
        assert_eq!(newest[0].step.label, crate::editor::EXTERNAL);
    }

    #[test]
    fn a_dry_run_after_an_outside_change_leaves_the_project_file_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let store = project_in(dir.path());
        add_bin(&store, "a");
        external_flip_ripple(store.path());
        let request = json!({"method": "bins.add", "params": {"path": "b"}, "dry_run": true});
        store
            .transact(|ed| {
                ed.apply(
                    Envelope::from_json(request).unwrap(),
                    &FakeEnv::default(),
                    None,
                )
            })
            .unwrap();
        assert!(store.history_db().meta().unwrap().external_dirty);
        assert!(!store.load_project().unwrap().project().settings.ripple_on);
    }

    #[test]
    fn a_non_sqlite_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.vidcut");
        std::fs::write(&path, "{}").unwrap();
        let err = Store::open(&path).unwrap().load().err().unwrap();
        assert_eq!(err.code, crate::error::ErrorCode::InvalidEdit);
    }

    #[test]
    fn identical_motion_paths_share_one_blob_row() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = crate::test_support::Fixture::new(&[(10.0, true)]);
        let samples = sample_path();
        let mut proj = fixture.editor.project().clone();
        proj.tracks[0].clips = vec![
            motion_clip("c1", 0, samples.clone()),
            motion_clip("c2", 20, samples),
        ]
        .into();
        let path = dir.path().join("p.vidcut");
        let store = Store::create(&path, &proj).unwrap();
        let (count, row_data): (i64, String) = store
            .with_conn(|c| {
                let count = c
                    .query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0))
                    .map_err(|e| sqlite_err(&path, e))?;
                let data = c
                    .query_row("SELECT data FROM clips WHERE id = 'c1'", [], |r| r.get(0))
                    .map_err(|e| sqlite_err(&path, e))?;
                Ok((count, data))
            })
            .unwrap();
        assert_eq!(count, 1);
        assert!(row_data.contains("\"$blob\""));
    }

    #[test]
    fn saving_one_changed_clip_leaves_other_clip_rows_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = crate::test_support::Fixture::new(&[(10.0, true)]);
        let samples = sample_path();
        let mut proj = fixture.editor.project().clone();
        proj.tracks[0].clips = vec![
            motion_clip("c1", 0, samples.clone()),
            motion_clip("c2", 20, samples),
        ]
        .into();
        let path = dir.path().join("p.vidcut");
        let store = Store::create(&path, &proj).unwrap();
        let mut ed = store.load().unwrap();
        store
            .with_conn(|c| {
                c.execute_batch(
                    "CREATE TEMP TABLE written (id TEXT);
                     CREATE TEMP TRIGGER tr_clip_ins AFTER INSERT ON clips
                     BEGIN INSERT INTO written VALUES (NEW.id); END;
                     CREATE TEMP TRIGGER tr_clip_upd AFTER UPDATE ON clips
                     BEGIN INSERT INTO written VALUES (NEW.id); END;",
                )
                .map_err(|e| sqlite_err(&path, e))?;
                Ok(())
            })
            .unwrap();
        let req = Envelope::from_json(
            json!({"method": "clip.set", "params": {"clips": "c1", "color": "teal"}}),
        )
        .unwrap();
        ed.apply(req, &FakeEnv::default(), None).unwrap();
        store.save(&mut ed).unwrap();
        let written: Vec<String> = store
            .with_conn(|c| {
                let mut stmt = c
                    .prepare("SELECT id FROM written")
                    .map_err(|e| sqlite_err(&path, e))?;
                let rows = stmt
                    .query_map([], |r| r.get::<_, String>(0))
                    .map_err(|e| sqlite_err(&path, e))?;
                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| sqlite_err(&path, e))
            })
            .unwrap();
        assert_eq!(written, vec!["c1"]);
    }

    #[test]
    fn moving_a_project_file_keeps_its_history_and_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        let a = crate::dirs::canonical(dir.path()).unwrap().join("a");
        let b = crate::dirs::canonical(dir.path()).unwrap().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let fixture = crate::test_support::Fixture::new(&[(4.0, true)]);
        let mut proj = fixture.editor.project().clone();
        proj.media[0].path = a.join("clip.mp4");
        let path_a = a.join("p.vidcut");
        let store = Store::create(&path_a, &proj).unwrap();
        add_bin(&store, "scenes");
        store.close();
        drop(store);
        let path_b = b.join("p.vidcut");
        std::fs::rename(&path_a, &path_b).unwrap();
        let moved = Store::open(&path_b).unwrap().load().unwrap();
        assert_eq!(
            (
                moved.project().media[0].path.clone(),
                moved.project().bins.clone(),
                moved.history_counts()
            ),
            (b.join("clip.mp4"), vec!["scenes".into()], (1, 0))
        );
    }

    #[test]
    fn gc_blobs_removes_unreferenced_blobs() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = crate::test_support::Fixture::new(&[(10.0, true)]);
        let mut proj = fixture.editor.project().clone();
        proj.tracks[0].clips = vec![motion_clip("c1", 0, sample_path())].into();
        let path = dir.path().join("p.vidcut");
        let store = Store::create(&path, &proj).unwrap();
        store
            .transact(|ed| {
                ed.apply(
                    Envelope::from_json(
                        json!({"method": "edit.delete", "params": {"clips": "c1"}}),
                    )
                    .unwrap(),
                    &FakeEnv::default(),
                    None,
                )
            })
            .unwrap();
        let count_after_gc: i64 = store
            .with_conn(|c| {
                let tx = c.transaction().unwrap();
                gc_blobs(&tx).unwrap();
                tx.commit().unwrap();
                c.query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0))
                    .map_err(|e| sqlite_err(&path, e))
            })
            .unwrap();
        assert_eq!(count_after_gc, 0);
    }

    #[test]
    fn two_stores_keep_both_transact_edits() {
        let dir = tempfile::tempdir().unwrap();
        let store = project_in(dir.path());
        let s1 = store.clone();
        let s2 = store.clone();
        add_bin(&s1, "one");
        add_bin(&s2, "two");
        let loaded = store.load().unwrap();
        assert_eq!(
            (loaded.project().bins.clone(), loaded.history_counts()),
            (vec!["one".into(), "two".into()], (2, 0))
        );
    }

    #[test]
    fn saving_a_clip_reinserts_its_blob_when_missing_from_db() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = crate::test_support::Fixture::new(&[(10.0, true)]);
        let mut proj = fixture.editor.project().clone();
        proj.tracks[0].clips = vec![motion_clip("c1", 0, sample_path())].into();
        let path = dir.path().join("p.vidcut");
        let store = Store::create(&path, &proj).unwrap();
        let mut ed = store.load().unwrap();
        store
            .with_conn(|c| {
                c.execute("DELETE FROM blobs", [])
                    .map_err(|e| sqlite_err(&path, e))?;
                Ok(())
            })
            .unwrap();
        let rename = Envelope::from_json(
            json!({"method": "clip.set", "params": {"clips": ["c1"], "name": "renamed"}}),
        )
        .unwrap();
        ed.apply(rename, &FakeEnv::default(), None).unwrap();
        store.save(&mut ed).unwrap();
        let blobs: i64 = store
            .with_conn(|c| {
                c.query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0))
                    .map_err(|e| sqlite_err(&path, e))
            })
            .unwrap();
        let reloaded = store.load().unwrap();
        let comp = reloaded.project().tracks[0].clips[0]
            .picture
            .as_deref()
            .unwrap()
            .composition
            .as_ref()
            .unwrap();
        let Tool::Tracker(tracker) = &comp.nodes[0].tool else {
            panic!("expected tracker");
        };
        let samples = &tracker.patterns[0].path.as_ref().unwrap().samples;
        assert_eq!((blobs, samples.len(), samples[0].frame), (1, 20, 0));
    }

    #[test]
    fn save_after_generation_bump_keeps_history_and_appends_edit() {
        let dir = tempfile::tempdir().unwrap();
        let store = project_in(dir.path());
        add_bin(&store, "a");
        add_bin(&store, "b");
        let mut ed = store.load().unwrap();
        store
            .with_conn(|c| {
                c.execute("UPDATE history_meta SET generation = 2 WHERE id = 1", [])
                    .map_err(|e| sqlite_err(store.path(), e))?;
                Ok(())
            })
            .unwrap();
        let add_c =
            Envelope::from_json(json!({"method": "bins.add", "params": {"path": "c"}})).unwrap();
        ed.apply(add_c, &FakeEnv::default(), None).unwrap();
        store.save(&mut ed).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(
            (loaded.history_counts(), loaded.project().bins.clone()),
            ((3, 0), vec!["a".into(), "b".into(), "c".into()])
        );
    }

    #[test]
    fn external_edit_after_moving_folder_has_no_spurious_path_diff() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::dirs::canonical(dir.path()).unwrap();
        let a = root.join("a");
        let b = root.join("b");
        std::fs::create_dir_all(&a).unwrap();
        let fixture = crate::test_support::Fixture::new(&[(4.0, true)]);
        let mut proj = fixture.editor.project().clone();
        proj.media[0].path = a.join("clip.mp4");
        let path_a = a.join("p.vidcut");
        let store = Store::create(&path_a, &proj).unwrap();
        store.close();
        drop(store);
        std::fs::rename(&a, &b).unwrap();
        let path_b = b.join("p.vidcut");
        external_flip_ripple(&path_b);
        let mut moved = Store::open(&path_b).unwrap().load().unwrap();
        let undo =
            Envelope::from_json(json!({"method": "history.undo", "params": {"steps": 1}})).unwrap();
        moved.apply(undo, &FakeEnv::default(), None).unwrap();
        assert_eq!(
            (
                moved.project().settings.ripple_on,
                moved.project().media[0].path.clone()
            ),
            (true, b.join("clip.mp4"))
        );
    }

    #[test]
    fn undoing_media_relink_after_moving_folder_uses_new_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::dirs::canonical(dir.path()).unwrap();
        let a = root.join("a");
        let b = root.join("b");
        std::fs::create_dir_all(&a).unwrap();
        let fixture = crate::test_support::Fixture::new(&[(4.0, true)]);
        let mut proj = fixture.editor.project().clone();
        proj.media[0].path = a.join("clip.mp4");
        let path_a = a.join("p.vidcut");
        let store = Store::create(&path_a, &proj).unwrap();
        let mut env = FakeEnv::default();
        env.files.insert(
            a.join("sub").join("clip.mp4"),
            crate::test_support::media_info(4.0, crate::time::Rational::new(25, 1).unwrap(), true),
        );
        let relink = Envelope::from_json(json!({
            "method": "media.relink",
            "params": {
                "media": "m1",
                "path": a.join("sub").join("clip.mp4").to_string_lossy()
            }
        }))
        .unwrap();
        store.transact(|ed| ed.apply(relink, &env, None)).unwrap();
        store.close();
        drop(store);
        std::fs::rename(&a, &b).unwrap();
        let mut moved = Store::open(&b.join("p.vidcut")).unwrap().load().unwrap();
        let undo =
            Envelope::from_json(json!({"method": "history.undo", "params": {"steps": 1}})).unwrap();
        moved.apply(undo, &FakeEnv::default(), None).unwrap();
        assert_eq!(moved.project().media[0].path, b.join("clip.mp4"));
    }

    #[test]
    fn lut_paths_in_change_relativize_and_resolve_across_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::dirs::canonical(dir.path()).unwrap();
        let a = root.join("a");
        let b = root.join("b");
        let mut c: Change = serde_json::from_value(json!({
            "fields": {
                "settings": {
                    "fields": {
                        "input_lut": {"set": {
                            "was": a.join("luts").join("base.cube").to_string_lossy(),
                            "is": a.join("luts").join("next.cube").to_string_lossy()
                        }},
                        "color_viewer_lut": {"set": {"was": "none", "is": {
                            "file": a.join("luts").join("view.cube").to_string_lossy()
                        }}}
                    }
                },
                "media": {
                    "items": {
                        "changed": {
                            "m1": {
                                "fields": {
                                    "input_lut": {"set": {
                                        "is": a.join("luts").join("clip.cube").to_string_lossy()
                                    }}
                                }
                            }
                        }
                    }
                }
            }
        }))
        .unwrap();
        relativize_change(&mut c, &a);
        let rel = serde_json::to_value(&c).unwrap();
        assert_eq!(
            (
                rel["fields"]["settings"]["fields"]["input_lut"]["set"]["was"].as_str(),
                rel["fields"]["settings"]["fields"]["color_viewer_lut"]["set"]["is"]["file"]
                    .as_str(),
                rel["fields"]["media"]["items"]["changed"]["m1"]["fields"]["input_lut"]["set"]
                    ["is"]
                    .as_str(),
            ),
            (
                Some("luts/base.cube"),
                Some("luts/view.cube"),
                Some("luts/clip.cube"),
            )
        );
        resolve_change(&mut c, &b);
        let res = serde_json::to_value(&c).unwrap();
        assert_eq!(
            (
                res["fields"]["settings"]["fields"]["input_lut"]["set"]["was"].as_str(),
                res["fields"]["settings"]["fields"]["color_viewer_lut"]["set"]["is"]["file"]
                    .as_str(),
                res["fields"]["media"]["items"]["changed"]["m1"]["fields"]["input_lut"]["set"]
                    ["is"]
                    .as_str(),
            ),
            (
                Some(b.join("luts").join("base.cube").to_string_lossy().as_ref()),
                Some(b.join("luts").join("view.cube").to_string_lossy().as_ref()),
                Some(b.join("luts").join("clip.cube").to_string_lossy().as_ref()),
            )
        );
    }

    #[test]
    fn stabilized_frames_offload_to_blobs_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = crate::test_support::Fixture::new(&[(10.0, true)]);
        let mut proj = fixture.editor.project().clone();
        let mut clip = motion_clip("c1", 0, Vec::new());
        clip.duration = 24;
        clip.picture = Some(
            Picture {
                stabilization: Some(Stabilization {
                    values: Stabilizing::default(),
                    off: false,
                    made: Some(Stabilized {
                        with: Stabilizing::default(),
                        first: 0,
                        frames: Arc::new(vec![[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]; 24]),
                    }),
                }),
                ..Picture::default()
            }
            .into(),
        );
        proj.tracks[0].clips = vec![clip].into();
        let path = dir.path().join("p.vidcut");
        let store = Store::create(&path, &proj).unwrap();
        let (blobs, raw_clip): (i64, String) = store
            .with_conn(|c| {
                let blobs = c
                    .query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0))
                    .map_err(|e| sqlite_err(&path, e))?;
                let raw = c
                    .query_row("SELECT data FROM clips WHERE id = 'c1'", [], |r| r.get(0))
                    .map_err(|e| sqlite_err(&path, e))?;
                Ok((blobs, raw))
            })
            .unwrap();
        let loaded = store.load().unwrap();
        let frames = &loaded.project().tracks[0].clips[0]
            .picture
            .as_deref()
            .unwrap()
            .stabilization
            .as_ref()
            .unwrap()
            .made
            .as_ref()
            .unwrap()
            .frames;
        assert_eq!(
            (blobs, raw_clip.contains("\"$blob\""), frames.len()),
            (1, true, 24)
        );
        assert_eq!(frames[0], [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn remote_journal_mode_uses_delete_without_wal_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.vidcut");
        let project = Project::new(Settings {
            auto_format: false,
            ..Settings::default()
        });
        let store = Store::create(&path, &project).unwrap();
        store
            .with_conn(|conn| {
                configure_journal(conn, true, true).map_err(|e| sqlite_err(&path, e))?;
                let mode: String = conn
                    .query_row("PRAGMA journal_mode", [], |r| r.get(0))
                    .map_err(|e| sqlite_err(&path, e))?;
                let sync_mode: i64 = conn
                    .query_row("PRAGMA synchronous", [], |r| r.get(0))
                    .map_err(|e| sqlite_err(&path, e))?;
                assert_eq!(
                    (mode.to_ascii_lowercase().as_str(), sync_mode),
                    ("delete", 2)
                );
                Ok(())
            })
            .unwrap();
        add_bin(&store, "remote-bin");
        assert!(!sidecar(&path, "-wal").exists());
        assert_eq!(store.load().unwrap().project().bins, vec!["remote-bin"]);
    }

    #[test]
    fn a_name_starting_with_a_wide_character_is_relative() {
        assert!(!windows_absolute("日本/a.mp4"));
    }

    #[cfg(windows)]
    #[test]
    fn a_path_from_the_root_names_nothing_on_windows() {
        assert!(absolute_elsewhere("/Users/x/a.mp4"));
        assert!(!absolute_elsewhere("//server/share/a.mp4"));
        assert!(!absolute_elsewhere(r"C:\x\a.mp4"));
    }

    #[cfg(not(windows))]
    #[test]
    fn a_drive_or_server_path_names_nothing_elsewhere() {
        assert!(absolute_elsewhere(r"C:\x\a.mp4"));
        assert!(absolute_elsewhere(r"\\server\share\a.mp4"));
        assert!(!absolute_elsewhere("/Users/x/a.mp4"));
    }
}
