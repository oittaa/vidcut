use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, Transaction, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::editor::{Editor, Entry};
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
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA temp_store = MEMORY;
PRAGMA wal_autocheckpoint = 1000;
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

#[derive(Serialize, Deserialize)]
struct MetaRow {
    settings: Settings,
    #[serde(default)]
    recent: Vec<String>,
    #[serde(default)]
    compound_path: Vec<OpenCompound>,
    #[serde(default)]
    tape_marks: TapeMarks,
    next_id: NextIds,
    #[serde(default)]
    bins: Vec<String>,
    #[serde(default)]
    groups: Vec<SyncGroup>,
    #[serde(default)]
    speakers: Speakers,
    #[serde(default)]
    queue: Vec<RenderJob>,
}

#[derive(Serialize, Deserialize)]
struct TimelineRow {
    id: String,
    name: String,
    #[serde(default)]
    bin: String,
    start_tc: i64,
    #[serde(default)]
    own: Option<TimelineSettings>,
    #[serde(default)]
    grade: Option<Grade>,
    #[serde(default)]
    render: RenderRange,
    #[serde(default)]
    clip_filter: ClipFilter,
    #[serde(default)]
    marks: Marks,
    #[serde(default)]
    markers: Vec<Marker>,
}

#[derive(Serialize, Deserialize)]
struct CompoundRow {
    id: String,
    name: String,
    #[serde(default)]
    bin: String,
    #[serde(default)]
    removed: bool,
    sequence_id: String,
    fps: crate::time::Rational,
    #[serde(default)]
    drop_frame: bool,
    #[serde(default)]
    start_tc: i64,
    frames: i64,
    #[serde(default)]
    marks: Marks,
    #[serde(default)]
    markers: Vec<Marker>,
    #[serde(default)]
    kept_otio: Option<Kept>,
}

#[derive(Serialize, Deserialize)]
struct TrackRow {
    id: String,
    kind: TrackKind,
    locked: bool,
    muted: bool,
    #[serde(default)]
    solo: bool,
    #[serde(default)]
    volume_db: f64,
    #[serde(default)]
    voice_isolation: Option<VoiceIsolation>,
    #[serde(default)]
    duck: Option<crate::model::Duck>,
    #[serde(default)]
    equalizer: Option<crate::model::TrackEqualizer>,
    #[serde(default)]
    dynamics: Option<crate::model::TrackDynamics>,
    video_enabled: bool,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    transitions: Vec<Transition>,
    #[serde(default = "on_true")]
    render_cache: bool,
}

#[derive(Serialize, Deserialize)]
struct SubtitleTrackRow {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    language: Option<String>,
    shown: bool,
    #[serde(default = "on_true")]
    destination: bool,
    locked: bool,
    #[serde(default)]
    style: SubtitleStyle,
}

fn on_true() -> bool {
    true
}

fn configure_conn(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA temp_store = MEMORY;")
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

pub(crate) fn open_conn(path: &Path) -> Result<Connection> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags).map_err(|e| open_err(path, e))?;
    configure_conn(&conn).map_err(|e| open_err(path, e))?;
    verify_db(&conn, path)?;
    Ok(conn)
}

fn open_ro_or_rw(path: &Path) -> Result<Connection> {
    if let Ok(conn) = open_conn(path) {
        return Ok(conn);
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags).map_err(|e| open_err(path, e))?;
    conn.busy_timeout(BUSY_TIMEOUT)
        .map_err(|e| open_err(path, e))?;
    verify_db(&conn, path)?;
    Ok(conn)
}

fn query_data_version(conn: &Connection, path: &Path) -> Result<i64> {
    conn.query_row("PRAGMA data_version", [], |r| r.get(0))
        .map_err(|e| open_err(path, e))
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
    added: Vec<(i64, Entry)>,
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
            configure_conn(&conn).map_err(|e| sqlite_err(temp.path(), e))?;
            conn.execute_batch(SCHEMA)
                .map_err(|e| sqlite_err(temp.path(), e))?;
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|e| sqlite_err(temp.path(), e))?;
            sync_project(&tx, &dir, None, project, None)?;
            HistoryDb::init_meta_in(&tx, project)?;
            clear_dirty(&tx, temp.path())?;
            tx.commit().map_err(|e| sqlite_err(temp.path(), e))?;
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .map_err(|e| sqlite_err(temp.path(), e))?;
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

    pub fn stamp(&self) -> Option<(u64, std::time::SystemTime)> {
        stamp_of(&self.path)
    }

    pub fn file_id(&self) -> Option<FileId> {
        file_id_of(&self.path)
    }

    pub fn data_version(&self) -> Result<i64> {
        self.with_conn(|conn| query_data_version(conn, &self.path))
    }

    pub fn checkpoint(&self) {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(conn) = guard.take() {
            let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        }
    }

    fn with_conn<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(open_conn(&self.path)?);
        }
        let conn = guard.as_mut().expect("connection opened");
        f(conn)
    }

    pub fn reload_if_changed(
        &self,
        last_id: &mut Option<FileId>,
        last_dv: &mut i64,
    ) -> Result<Option<Editor>> {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let current_id = file_id_of(&self.path);
        let replaced = *last_id != current_id;
        if replaced {
            *guard = None;
            *last_id = current_id;
        }
        if guard.is_none() {
            *guard = Some(open_conn(&self.path)?);
        }
        let conn = guard.as_ref().expect("connection opened");
        let dv = query_data_version(conn, &self.path)?;
        if !replaced && dv == *last_dv {
            return Ok(None);
        }
        let editor = self.load_in(conn)?;
        *last_dv = dv;
        Ok(Some(editor))
    }

    pub fn mutate_live<T>(
        &self,
        last_id: &mut Option<FileId>,
        last_dv: &mut i64,
        editor: &mut Editor,
        opened: u64,
        apply: impl FnOnce(&mut Editor) -> Result<T>,
    ) -> Result<LiveOutcome<T>> {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let current_id = file_id_of(&self.path);
        let replaced = *last_id != current_id;
        if replaced {
            *guard = None;
            *last_id = current_id;
        }
        if guard.is_none() {
            *guard = Some(open_conn(&self.path)?);
        }
        let conn = guard.as_mut().expect("connection opened");
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| sqlite_err(&self.path, e))?;
        let dv = query_data_version(&tx, &self.path)?;
        let mut refreshed = false;
        if replaced || dv != *last_dv {
            let mut loaded = self.load_in(&tx)?;
            loaded.opened_at(opened);
            *editor = loaded;
            *last_dv = dv;
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
                *last_dv = query_data_version(conn, &self.path)?;
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
        self.with_conn(|conn| self.load_in(conn))
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
        let conn = open_ro_or_rw(&self.path)?;
        let project =
            load_project_from_conn(&conn, &self.path, self.dir(), Some(&self.blob_hashes))?;
        let meta = HistoryDb::read_meta_in(&conn)?;
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
        let meta = db.meta().unwrap_or(HistoryMeta {
            undo: 0,
            redo: 0,
            steps: 0,
            next: 1,
            generation: 1,
            external_dirty: false,
        });
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
        let meta = db.meta().unwrap_or(HistoryMeta {
            undo: 0,
            redo: 0,
            steps: 0,
            next: 1,
            generation: 1,
            external_dirty: false,
        });
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
            (out, saved.compact)
        };
        if compact {
            let _ = self.compact();
        }
        Ok(out)
    }

    pub fn save(&self, editor: &mut Editor) -> Result<Saved> {
        self.with_conn(|conn| {
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|e| sqlite_err(&self.path, e))?;
            let (saved, commit) = self.save_in_tx(&tx, editor)?;
            tx.commit().map_err(|e| sqlite_err(&self.path, e))?;
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
        let new_meta = if editor.has_db() && editor.db_generation() == meta.generation {
            if moved.is_empty() {
                meta
            } else {
                let (m, inserted) = HistoryDb::append_in(tx, &meta, moved)?;
                added = inserted;
                m
            }
        } else {
            let h = editor.held();
            let mut base = serde_json::to_value(editor.project()).map_err(json_err)?;
            for e in h.undo.iter().rev() {
                e.change.apply(&mut base, false).map_err(Error::internal)?;
            }
            HistoryDb::write_whole_in(tx, &base, h, meta.generation + 1, None, true)?
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
    let (rev, edited, meta_json): (u64, Option<i64>, String) = conn
        .query_row("SELECT rev, edited, data FROM meta WHERE id = 1", [], |r| {
            Ok((r.get::<_, i64>(0)?.max(0) as u64, r.get(1)?, r.get(2)?))
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
                .remove(&(owner.clone(), tr.id.clone()))
                .unwrap_or_default();
            tracks_by.entry(owner).or_default().push(Track {
                id: tr.id,
                kind: tr.kind,
                clips: clips.into(),
                locked: tr.locked,
                muted: tr.muted,
                solo: tr.solo,
                volume_db: tr.volume_db,
                voice_isolation: tr.voice_isolation,
                duck: tr.duck,
                equalizer: tr.equalizer,
                dynamics: tr.dynamics,
                video_enabled: tr.video_enabled,
                name: tr.name,
                transitions: tr.transitions,
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
                .remove(&(tl_id.clone(), sr.id.clone()))
                .unwrap_or_default();
            subs_by.entry(tl_id).or_default().push(SubtitleTrack {
                id: sr.id,
                name: sr.name,
                language: sr.language,
                shown: sr.shown,
                destination: sr.destination,
                locked: sr.locked,
                style: sr.style,
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
            let subtitles = subs_by.remove(&tr.id).unwrap_or_default();
            out.push(Timeline {
                id: tr.id,
                name: tr.name,
                bin: tr.bin,
                start_tc: tr.start_tc,
                own: tr.own,
                grade: tr.grade,
                render: tr.render,
                clip_filter: tr.clip_filter,
                tracks: tracks.into(),
                marks: tr.marks,
                markers: tr.markers,
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
                id: kr.id,
                name: kr.name,
                bin: kr.bin,
                removed: kr.removed,
                sequence_id: kr.sequence_id,
                fps: kr.fps,
                drop_frame: kr.drop_frame,
                start_tc: kr.start_tc,
                frames: kr.frames,
                tracks: tracks.into(),
                marks: kr.marks,
                markers: kr.markers,
                kept_otio: kr.kept_otio,
            });
        }
        out
    };

    let mut project = Project {
        rev,
        edited: edited.map(|e| e.max(0) as u64),
        settings: meta.settings,
        media: media.into(),
        tracks: SharedVec::new(),
        compounds: compounds.into(),
        compound_path: meta.compound_path,
        marks: Marks::default(),
        tape_marks: meta.tape_marks,
        markers: Vec::new(),
        subtitles: SharedVec::new(),
        timelines: timelines.into(),
        recent: meta.recent,
        next_id: meta.next_id,
        bins: meta.bins,
        groups: meta.groups,
        speakers: meta.speakers,
        queue: meta.queue,
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
    let meta_changed = match old {
        None => true,
        Some(old) => {
            old.rev != new.rev
                || old.edited != new.edited
                || old.settings != new.settings
                || old.recent != new.recent
                || old.compound_path != new.compound_path
                || old.tape_marks != new.tape_marks
                || old.next_id != new.next_id
                || old.bins != new.bins
                || old.groups != new.groups
                || old.speakers != new.speakers
                || old.queue != new.queue
        }
    };
    if meta_changed {
        let mut settings = new.settings.clone();
        relativize_settings(&mut settings, dir);
        let row = MetaRow {
            settings,
            recent: new.recent.clone(),
            compound_path: new.compound_path.clone(),
            tape_marks: new.tape_marks.clone(),
            next_id: new.next_id.clone(),
            bins: new.bins.clone(),
            groups: new.groups.clone(),
            speakers: new.speakers.clone(),
            queue: new.queue.clone(),
        };
        let data = serde_json::to_string(&row).map_err(json_err)?;
        tx.execute(
            "INSERT OR REPLACE INTO meta (id, rev, edited, data) VALUES (1, ?1, ?2, ?3)",
            params![new.rev as i64, new.edited.map(|e| e as i64), data],
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
        let (new_tracks, new_marks, new_markers, new_subs) = timeline_sheet(new, new_t);
        let old_entry = old_map.get(new_t.id.as_str()).copied();
        let old_sheet = old_entry.map(|(old_ord, old_t)| {
            (
                old_ord,
                old_t,
                timeline_sheet(old.expect("old present"), old_t),
            )
        });
        let header_dirty = match old_sheet {
            None => true,
            Some((old_ord, old_t, (_, old_marks, old_markers, _))) => {
                old_ord != ord
                    || old_t.name != new_t.name
                    || old_t.bin != new_t.bin
                    || old_t.start_tc != new_t.start_tc
                    || old_t.own != new_t.own
                    || old_t.grade != new_t.grade
                    || old_t.render != new_t.render
                    || old_t.clip_filter != new_t.clip_filter
                    || old_marks != new_marks
                    || old_markers != new_markers
            }
        };
        if header_dirty {
            let row = TimelineRow {
                id: new_t.id.clone(),
                name: new_t.name.clone(),
                bin: new_t.bin.clone(),
                start_tc: new_t.start_tc,
                own: new_t.own,
                grade: new_t.grade.clone(),
                render: new_t.render,
                clip_filter: new_t.clip_filter.clone(),
                marks: new_marks.clone(),
                markers: new_markers.clone(),
            };
            let data = serde_json::to_string(&row).map_err(json_err)?;
            tx.execute(
                "INSERT OR REPLACE INTO timelines (id, ord, name, bin, data) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![new_t.id, ord as i64, new_t.name, new_t.bin, data],
            )
            .map_err(|e| Error::internal(e.to_string()))?;
        }
        let owner = format!("t:{}", new_t.id);
        sync_tracks(
            tx,
            &owner,
            old_sheet.map(|(_, _, (t, _, _, _))| t),
            new_tracks,
            blob_hashes,
        )?;
        sync_subtitles(
            tx,
            &new_t.id,
            old_sheet.map(|(_, _, (_, _, _, s))| s),
            new_subs,
        )?;
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
        let (new_tracks, new_marks, new_markers) = compound_sheet(new, new_k);
        let old_entry = old_map.get(new_k.id.as_str()).copied();
        let old_sheet = old_entry.map(|(old_ord, old_k)| {
            (
                old_ord,
                old_k,
                compound_sheet(old.expect("old present"), old_k),
            )
        });
        let header_dirty = match old_sheet {
            None => true,
            Some((old_ord, old_k, (_, old_marks, old_markers))) => {
                old_ord != ord
                    || old_k.name != new_k.name
                    || old_k.bin != new_k.bin
                    || old_k.removed != new_k.removed
                    || old_k.sequence_id != new_k.sequence_id
                    || old_k.fps != new_k.fps
                    || old_k.drop_frame != new_k.drop_frame
                    || old_k.start_tc != new_k.start_tc
                    || old_k.frames != new_k.frames
                    || old_k.kept_otio != new_k.kept_otio
                    || old_marks != new_marks
                    || old_markers != new_markers
            }
        };
        if header_dirty {
            let row = CompoundRow {
                id: new_k.id.clone(),
                name: new_k.name.clone(),
                bin: new_k.bin.clone(),
                removed: new_k.removed,
                sequence_id: new_k.sequence_id.clone(),
                fps: new_k.fps,
                drop_frame: new_k.drop_frame,
                start_tc: new_k.start_tc,
                frames: new_k.frames,
                marks: new_marks.clone(),
                markers: new_markers.clone(),
                kept_otio: new_k.kept_otio.clone(),
            };
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
            old_sheet.map(|(_, _, (t, _, _))| t),
            new_tracks,
            blob_hashes,
        )?;
    }
    Ok(())
}

fn track_meta_eq(a: &Track, b: &Track) -> bool {
    a.kind == b.kind
        && a.locked == b.locked
        && a.muted == b.muted
        && a.solo == b.solo
        && a.volume_db == b.volume_db
        && a.voice_isolation == b.voice_isolation
        && a.duck == b.duck
        && a.equalizer == b.equalizer
        && a.dynamics == b.dynamics
        && a.video_enabled == b.video_enabled
        && a.name == b.name
        && a.transitions == b.transitions
        && a.render_cache == b.render_cache
}

fn upsert_track(tx: &Transaction<'_>, owner: &str, ord: usize, tr: &Track) -> Result<()> {
    let row = TrackRow {
        id: tr.id.clone(),
        kind: tr.kind,
        locked: tr.locked,
        muted: tr.muted,
        solo: tr.solo,
        volume_db: tr.volume_db,
        voice_isolation: tr.voice_isolation,
        duck: tr.duck.clone(),
        equalizer: tr.equalizer.clone(),
        dynamics: tr.dynamics,
        video_enabled: tr.video_enabled,
        name: tr.name.clone(),
        transitions: tr.transitions.clone(),
        render_cache: tr.render_cache,
    };
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
            if old_entry
                .is_none_or(|(old_ord, old_tr)| old_ord != ord || !track_meta_eq(old_tr, new_tr))
            {
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

fn subtitle_meta_eq(a: &SubtitleTrack, b: &SubtitleTrack) -> bool {
    a.name == b.name
        && a.language == b.language
        && a.shown == b.shown
        && a.destination == b.destination
        && a.locked == b.locked
        && a.style == b.style
}

fn upsert_subtitle_track(
    tx: &Transaction<'_>,
    tl_id: &str,
    ord: usize,
    sr: &SubtitleTrack,
) -> Result<()> {
    let row = SubtitleTrackRow {
        id: sr.id.clone(),
        name: sr.name.clone(),
        language: sr.language.clone(),
        shown: sr.shown,
        destination: sr.destination,
        locked: sr.locked,
        style: sr.style.clone(),
    };
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
            if old_entry
                .is_none_or(|(old_ord, old_s)| old_ord != ord || !subtitle_meta_eq(old_s, new_s))
            {
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
    let resolve = |file: &mut PathBuf| {
        if !absolute_anywhere(file) {
            *file = dir.join(native(file));
        }
    };
    for m in media {
        resolve(&mut m.path);
        if let Some(lut) = m.input_lut.as_mut() {
            resolve(lut);
        }
    }
    if let Some(p) = settings.input_lut.as_mut() {
        resolve(p);
    }
    if let Some(p) = settings.output_lut.as_mut() {
        resolve(p);
    }
    if let Some(p) = settings.video_monitor_lut.as_mut() {
        resolve(p);
    }
    for lut in [&mut settings.color_viewer_lut, &mut settings.scopes_lut] {
        if let crate::model::MonitorLut::File(path) = lut {
            resolve(path);
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
        Settings, Tracker,
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
    fn an_incremental_save_writes_only_changed_clips_and_deduplicates_motion_blobs() {
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
        let count: i64 = store
            .with_conn(|c| {
                c.query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0))
                    .map_err(|e| sqlite_err(&path, e))
            })
            .unwrap();
        assert_eq!(count, 1);
        store
            .transact(|ed| {
                ed.apply(
                    Envelope::from_json(
                        json!({"method": "clip.set", "params": {"clips": "c1", "color": "teal"}}),
                    )
                    .unwrap(),
                    &FakeEnv::default(),
                    None,
                )
            })
            .unwrap();
        let reloaded = store.load().unwrap();
        assert_eq!(
            reloaded.project().clip("c1").unwrap().color.as_deref(),
            Some("teal")
        );
        assert_eq!(reloaded.project().clip("c2"), proj.clip("c2"));
    }

    #[test]
    fn moving_a_project_file_keeps_its_history_and_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        let a = crate::dirs::canonical(dir.path()).unwrap().join("a");
        let b = crate::dirs::canonical(dir.path()).unwrap().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let mut fixture = crate::test_support::Fixture::new(&[(4.0, true)]);
        let mut proj = fixture.editor.project().clone();
        proj.media[0].path = a.join("clip.mp4");
        let path_a = a.join("p.vidcut");
        let store = Store::create(&path_a, &proj).unwrap();
        add_bin(&store, "scenes");
        store.checkpoint();
        drop(store);
        let path_b = b.join("p.vidcut");
        std::fs::rename(&path_a, &path_b).unwrap();
        let moved = Store::open(&path_b).unwrap().load().unwrap();
        assert_eq!(
            (
                moved.project().media[0].path.clone(),
                moved.history_counts()
            ),
            (b.join("clip.mp4"), (1, 0))
        );
        fixture.ok(json!({"method": "bins.add", "params": {"path": "scenes"}}));
        assert_eq!(moved.project().bins, fixture.editor.project().bins);
    }

    #[test]
    fn compaction_removes_unreferenced_blobs() {
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
    fn concurrent_transact_writers_keep_both_edits() {
        let dir = tempfile::tempdir().unwrap();
        let store = project_in(dir.path());
        let s1 = store.clone();
        let s2 = store.clone();
        std::thread::scope(|scope| {
            scope.spawn(|| add_bin(&s1, "one"));
            scope.spawn(|| add_bin(&s2, "two"));
        });
        let loaded = store.load().unwrap();
        let mut bins = loaded.project().bins.clone();
        bins.sort();
        assert_eq!(
            (bins, loaded.history_counts()),
            (vec!["one".into(), "two".into()], (2, 0))
        );
    }

    #[test]
    fn motion_samples_survive_gain_edit_missing_blob_and_undo() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = crate::test_support::Fixture::new(&[(10.0, true)]);
        let mut proj = fixture.editor.project().clone();
        let clip = motion_clip("c1", 0, sample_path());
        let expected_pic = clip.picture.clone();
        proj.tracks[0].clips = vec![clip].into();
        let path = dir.path().join("p.vidcut");
        let store = Store::create(&path, &proj).unwrap();
        let mut ed = store.load().unwrap();
        let rename = Envelope::from_json(
            json!({"method": "clip.set", "params": {"clips": ["c1"], "name": "one"}}),
        )
        .unwrap();
        ed.apply(rename, &FakeEnv::default(), None).unwrap();
        store.save(&mut ed).unwrap();
        store
            .with_conn(|c| {
                c.execute("DELETE FROM blobs", [])
                    .map_err(|e| sqlite_err(&path, e))?;
                Ok(())
            })
            .unwrap();
        let rename2 = Envelope::from_json(
            json!({"method": "clip.set", "params": {"clips": ["c1"], "name": "two"}}),
        )
        .unwrap();
        ed.apply(rename2, &FakeEnv::default(), None).unwrap();
        store.save(&mut ed).unwrap();
        let after_edit = store.load().unwrap();
        assert_eq!(
            after_edit.project().tracks[0].clips[0].picture,
            expected_pic
        );
        let undo =
            Envelope::from_json(json!({"method": "history.undo", "params": {"steps": 1}})).unwrap();
        ed.apply(undo, &FakeEnv::default(), None).unwrap();
        store.save(&mut ed).unwrap();
        let after_undo = store.load().unwrap();
        assert_eq!(
            after_undo.project().tracks[0].clips[0].picture,
            expected_pic
        );
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
