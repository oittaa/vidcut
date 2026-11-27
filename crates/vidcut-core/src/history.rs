use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value;

use crate::editor::{Chain, Entry, HISTORY_CAP, HISTORY_ENTRIES_KEPT, History, Moved};
use crate::error::{Error, Result};
use crate::lru::Lru;
use crate::model::Project;
use crate::steps::{Branch, Mark, Node, Step, Steps, StepsCache, Tree};

const KIND_STEP: i64 = 0;
const KIND_UNDO: i64 = 1;
const KIND_REDO: i64 = 2;
const KIND_MARK: i64 = 3;
const KIND_UNMARK: i64 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoryMeta {
    pub undo: usize,
    pub redo: usize,
    pub steps: usize,
    pub next: u64,
    pub generation: u64,
    pub external_dirty: bool,
}

pub(crate) type EntryCache = Mutex<Lru<(u64, i64), Entry, HISTORY_ENTRIES_KEPT>>;

pub(crate) fn due(meta: &HistoryMeta) -> bool {
    meta.steps > HISTORY_CAP
}

static COMPACTING: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

pub(crate) fn compress_bytes(raw: &[u8]) -> Result<Vec<u8>> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::fast());
    enc.write_all(raw)
        .map_err(|e| Error::internal(e.to_string()))?;
    enc.finish().map_err(|e| Error::internal(e.to_string()))
}

pub(crate) fn decompress_bytes(compressed: &[u8]) -> Result<Vec<u8>> {
    let mut dec = ZlibDecoder::new(compressed);
    let mut out = Vec::new();
    dec.read_to_end(&mut out)
        .map_err(|e| Error::internal(format!("corrupt compressed project data: {e}")))?;
    Ok(out)
}

fn db_err(path: &Path, e: rusqlite::Error) -> Error {
    Error::internal(format!("{}: {e}", path.display()))
}

fn json_err(e: serde_json::Error) -> Error {
    Error::internal(e.to_string())
}

pub(crate) struct HistoryDb {
    path: PathBuf,
    conn: Mutex<Option<Connection>>,
}

impl std::fmt::Debug for HistoryDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistoryDb")
            .field("path", &self.path)
            .finish()
    }
}

impl Clone for HistoryDb {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            conn: Mutex::new(None),
        }
    }
}

impl HistoryDb {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            conn: Mutex::new(None),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn close(&self) {
        *self.conn.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn dir(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new("/"))
    }

    fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction().map_err(|e| db_err(&self.path, e))?;
            let out = f(&tx)?;
            tx.commit().map_err(|e| db_err(&self.path, e))?;
            Ok(out)
        })
    }

    fn with_conn_mut<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(crate::store::open_conn(&self.path)?);
        }
        let conn = guard.as_mut().expect("connection opened");
        f(conn)
    }

    pub(crate) fn init_meta_in(tx: &Transaction<'_>, base: &Project) -> Result<()> {
        let raw = serde_json::to_vec(base).map_err(json_err)?;
        let blob = compress_bytes(&raw)?;
        tx.execute(
            "INSERT OR REPLACE INTO history_meta \
             (id, undo_count, redo_count, total_steps, next_step_id, generation, external_dirty) \
             VALUES (1, 0, 0, 0, 1, 1, 0)",
            [],
        )
        .map_err(|e| Error::internal(e.to_string()))?;
        tx.execute(
            "INSERT OR REPLACE INTO history_base (id, base_json) VALUES (1, ?1)",
            params![blob],
        )
        .map_err(|e| Error::internal(e.to_string()))?;
        Ok(())
    }

    pub(crate) fn read_meta_in(conn: &Connection) -> Result<HistoryMeta> {
        conn.query_row(
            "SELECT undo_count, redo_count, total_steps, next_step_id, generation, external_dirty \
             FROM history_meta WHERE id = 1",
            [],
            |r| {
                Ok(HistoryMeta {
                    undo: r.get::<_, i64>(0)?.max(0) as usize,
                    redo: r.get::<_, i64>(1)?.max(0) as usize,
                    steps: r.get::<_, i64>(2)?.max(0) as usize,
                    next: r.get::<_, i64>(3)?.max(1) as u64,
                    generation: r.get::<_, i64>(4)?.max(1) as u64,
                    external_dirty: r.get::<_, i64>(5)? != 0,
                })
            },
        )
        .map_err(|e| Error::invalid(format!("invalid vidcut history metadata: {e}")))
    }

    pub(crate) fn meta(&self) -> Result<HistoryMeta> {
        self.read(Self::read_meta_in)
    }

    pub(crate) fn append_in(
        tx: &Transaction<'_>,
        dir: &Path,
        meta: &HistoryMeta,
        moved: &[Moved],
    ) -> Result<(HistoryMeta, Vec<(i64, Entry)>)> {
        let mut t = meta.clone();
        let mut added = Vec::new();
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO history_events \
                     (kind, at_pos, step_id, label, step_meta, change_delta) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .map_err(|e| Error::internal(e.to_string()))?;
            for m in moved {
                match m {
                    Moved::Step(e) => {
                        let mut e = (**e).clone();
                        t.undo += 1;
                        t.redo = 0;
                        t.steps += 1;
                        if e.step.id.is_none() {
                            e.step.id = Some(t.next);
                            t.next += 1;
                        }
                        let mut rel_change = e.change.clone();
                        crate::store::relativize_change(&mut rel_change, dir);
                        let step_json = serde_json::to_string(&e.step).map_err(json_err)?;
                        let change_raw = serde_json::to_vec(&rel_change).map_err(json_err)?;
                        let change_blob = compress_bytes(&change_raw)?;
                        stmt.execute(params![
                            KIND_STEP,
                            t.undo as i64,
                            e.step.id.map(|id| id as i64),
                            &e.step.label,
                            step_json,
                            change_blob
                        ])
                        .map_err(|err| Error::internal(err.to_string()))?;
                        let seq = tx.last_insert_rowid();
                        added.push((seq, e));
                    }
                    Moved::Undo(n) => {
                        let n = (*n as usize).min(t.undo);
                        t.undo -= n;
                        t.redo += n;
                        stmt.execute(params![
                            KIND_UNDO,
                            n as i64,
                            Option::<i64>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<Vec<u8>>::None
                        ])
                        .map_err(|err| Error::internal(err.to_string()))?;
                    }
                    Moved::Redo(n) => {
                        let n = (*n as usize).min(t.redo);
                        t.redo -= n;
                        t.undo += n;
                        stmt.execute(params![
                            KIND_REDO,
                            n as i64,
                            Option::<i64>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<Vec<u8>>::None
                        ])
                        .map_err(|err| Error::internal(err.to_string()))?;
                    }
                    Moved::Mark(m) => {
                        let mark_json = serde_json::to_string(m).map_err(json_err)?;
                        stmt.execute(params![
                            KIND_MARK,
                            Option::<i64>::None,
                            m.step as i64,
                            Option::<String>::None,
                            mark_json,
                            Option::<Vec<u8>>::None
                        ])
                        .map_err(|err| Error::internal(err.to_string()))?;
                    }
                    Moved::Unmark(id) => {
                        stmt.execute(params![
                            KIND_UNMARK,
                            Option::<i64>::None,
                            *id as i64,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<Vec<u8>>::None
                        ])
                        .map_err(|err| Error::internal(err.to_string()))?;
                    }
                }
            }
        }
        tx.execute(
            "UPDATE history_meta \
             SET undo_count = ?1, redo_count = ?2, total_steps = ?3, next_step_id = ?4 \
             WHERE id = 1",
            params![t.undo as i64, t.redo as i64, t.steps as i64, t.next as i64],
        )
        .map_err(|e| Error::internal(e.to_string()))?;
        Ok((t, added))
    }

    pub(crate) fn write_whole_in(
        tx: &Transaction<'_>,
        base: &Value,
        history: &History,
        generation: u64,
        next_override: Option<u64>,
        assign: bool,
    ) -> Result<HistoryMeta> {
        let mut line_of: Vec<Entry> = history.undo.clone();
        line_of.extend(history.redo.iter().rev().cloned());
        let mut next = next_override
            .unwrap_or(1)
            .max(biggest(&line_of, &history.branches) + 1);
        let mut steps = 0usize;
        tx.execute("DELETE FROM history_events", [])
            .map_err(|e| Error::internal(e.to_string()))?;
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO history_events \
                     (kind, at_pos, step_id, label, step_meta, change_delta) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .map_err(|e| Error::internal(e.to_string()))?;
            emit_steps(
                &mut stmt,
                0,
                &line_of,
                &history.branches,
                &mut steps,
                &mut next,
                assign,
            )?;
            if !history.redo.is_empty() {
                stmt.execute(params![
                    KIND_UNDO,
                    history.redo.len() as i64,
                    Option::<i64>::None,
                    Option::<String>::None,
                    Option::<String>::None,
                    Option::<Vec<u8>>::None
                ])
                .map_err(|e| Error::internal(e.to_string()))?;
            }
            for m in &history.marks {
                let mark_json = serde_json::to_string(m).map_err(json_err)?;
                stmt.execute(params![
                    KIND_MARK,
                    Option::<i64>::None,
                    m.step as i64,
                    Option::<String>::None,
                    mark_json,
                    Option::<Vec<u8>>::None
                ])
                .map_err(|e| Error::internal(e.to_string()))?;
            }
        }
        let base_raw = serde_json::to_vec(base).map_err(json_err)?;
        let base_blob = compress_bytes(&base_raw)?;
        let meta = HistoryMeta {
            undo: history.undo.len(),
            redo: history.redo.len(),
            steps,
            next,
            generation,
            external_dirty: false,
        };
        tx.execute(
            "INSERT OR REPLACE INTO history_meta \
             (id, undo_count, redo_count, total_steps, next_step_id, generation, external_dirty) \
             VALUES (1, ?1, ?2, ?3, ?4, ?5, 0)",
            params![
                meta.undo as i64,
                meta.redo as i64,
                meta.steps as i64,
                meta.next as i64,
                meta.generation as i64,
            ],
        )
        .map_err(|e| Error::internal(e.to_string()))?;
        tx.execute(
            "INSERT OR REPLACE INTO history_base (id, base_json) VALUES (1, ?1)",
            params![base_blob],
        )
        .map_err(|e| Error::internal(e.to_string()))?;
        Ok(meta)
    }

    pub(crate) fn top(
        &self,
        n: usize,
        m: usize,
        cache: Option<&EntryCache>,
    ) -> Result<(Vec<Entry>, Vec<Entry>)> {
        self.read(|conn| self.top_in(conn, n, m, cache))
    }

    fn top_in(
        &self,
        conn: &Connection,
        n: usize,
        m: usize,
        cache: Option<&EntryCache>,
    ) -> Result<(Vec<Entry>, Vec<Entry>)> {
        let meta = Self::read_meta_in(conn)?;
        let n = n.min(meta.undo);
        let m = m.min(meta.redo);
        let d = meta.undo;
        let mut undo = Vec::with_capacity(n);
        let mut before_seq = i64::MAX;
        for offset in 0..n {
            let pos = (d - offset) as i64;
            let seq: i64 = conn
                .query_row(
                    "SELECT seq FROM history_events \
                     WHERE kind = 0 AND at_pos = ?1 AND seq < ?2 \
                     ORDER BY seq DESC LIMIT 1",
                    params![pos, before_seq],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| db_err(&self.path, e))?
                .ok_or_else(|| Error::internal("fewer steps to undo than the file says"))?;
            undo.push(read_entry_cached(
                conn,
                &self.path,
                self.dir(),
                meta.generation,
                seq,
                cache,
            )?);
            before_seq = seq;
        }
        let mut redo = Vec::with_capacity(m);
        if m > 0 {
            let first_pos = (d + 1) as i64;
            let mut since: i64 = conn
                .query_row(
                    "SELECT seq FROM history_events \
                     WHERE kind = 0 AND at_pos = ?1 \
                     ORDER BY seq DESC LIMIT 1",
                    params![first_pos],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| db_err(&self.path, e))?
                .ok_or_else(|| Error::internal("fewer steps to redo than the file says"))?;
            redo.push(read_entry_cached(
                conn,
                &self.path,
                self.dir(),
                meta.generation,
                since,
                cache,
            )?);
            for pos in d + 2..=d + m {
                let seq: i64 = conn
                    .query_row(
                        "SELECT seq FROM history_events \
                         WHERE kind = 0 AND at_pos = ?1 AND seq > ?2 \
                         ORDER BY seq DESC LIMIT 1",
                        params![pos as i64, since],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|e| db_err(&self.path, e))?
                    .ok_or_else(|| Error::internal("fewer steps to redo than the file says"))?;
                redo.push(read_entry_cached(
                    conn,
                    &self.path,
                    self.dir(),
                    meta.generation,
                    seq,
                    cache,
                )?);
                since = seq;
            }
        }
        Ok((undo, redo))
    }

    pub(crate) fn top_labels(&self, n: usize, m: usize) -> Result<(Vec<String>, Vec<String>)> {
        self.read(|conn| {
            let meta = Self::read_meta_in(conn)?;
            let n = n.min(meta.undo);
            let m = m.min(meta.redo);
            let d = meta.undo;
            let mut undo = Vec::with_capacity(n);
            let mut before_seq = i64::MAX;
            for offset in 0..n {
                let pos = (d - offset) as i64;
                let (seq, label): (i64, String) = conn
                    .query_row(
                        "SELECT seq, label FROM history_events \
                         WHERE kind = 0 AND at_pos = ?1 AND seq < ?2 \
                         ORDER BY seq DESC LIMIT 1",
                        params![pos, before_seq],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .map_err(|e| db_err(&self.path, e))?
                    .ok_or_else(|| Error::internal("fewer steps to undo than the file says"))?;
                undo.push(label);
                before_seq = seq;
            }
            let mut redo = Vec::with_capacity(m);
            if m > 0 {
                let first_pos = (d + 1) as i64;
                let (mut since, label): (i64, String) = conn
                    .query_row(
                        "SELECT seq, label FROM history_events \
                         WHERE kind = 0 AND at_pos = ?1 \
                         ORDER BY seq DESC LIMIT 1",
                        params![first_pos],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .map_err(|e| db_err(&self.path, e))?
                    .ok_or_else(|| Error::internal("fewer steps to redo than the file says"))?;
                redo.push(label);
                for pos in d + 2..=d + m {
                    let (seq, label): (i64, String) = conn
                        .query_row(
                            "SELECT seq, label FROM history_events \
                             WHERE kind = 0 AND at_pos = ?1 AND seq > ?2 \
                             ORDER BY seq DESC LIMIT 1",
                            params![pos as i64, since],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .optional()
                        .map_err(|e| db_err(&self.path, e))?
                        .ok_or_else(|| Error::internal("fewer steps to redo than the file says"))?;
                    redo.push(label);
                    since = seq;
                }
            }
            Ok((undo, redo))
        })
    }

    pub(crate) fn between(
        &self,
        lo: usize,
        hi: usize,
        cache: Option<&EntryCache>,
    ) -> Result<Vec<Entry>> {
        self.read(|conn| {
            let d = Self::read_meta_in(conn)?.undo;
            if hi <= d {
                let (undo, _) = self.top_in(conn, d - lo, 0, cache)?;
                Ok(undo.into_iter().skip(d - hi).rev().collect())
            } else {
                let (_, redo) = self.top_in(conn, 0, hi - d, cache)?;
                Ok(redo.into_iter().skip(lo - d).collect())
            }
        })
    }

    pub(crate) fn steps_now(&self, cache: &mut StepsCache) -> Result<bool> {
        self.read(|conn| {
            let meta = Self::read_meta_in(conn)?;
            let kept = cache.path == self.path && cache.generation == meta.generation;
            let (mut steps, after_seq) = if kept {
                (std::mem::take(&mut cache.steps), cache.last_seq)
            } else {
                (Steps::default(), 0)
            };
            let mut stmt = conn
                .prepare_cached(
                    "SELECT seq, kind, at_pos, step_id, step_meta \
                     FROM history_events WHERE seq > ?1 ORDER BY seq ASC",
                )
                .map_err(|e| db_err(&self.path, e))?;
            let mut rows = stmt
                .query(params![after_seq])
                .map_err(|e| db_err(&self.path, e))?;
            let mut last_seq = after_seq;
            while let Some(r) = rows.next().map_err(|e| db_err(&self.path, e))? {
                last_seq = r.get(0).map_err(|e| db_err(&self.path, e))?;
                let kind: i64 = r.get(1).map_err(|e| db_err(&self.path, e))?;
                let at_pos: Option<i64> = r.get(2).map_err(|e| db_err(&self.path, e))?;
                let step_id: Option<i64> = r.get(3).map_err(|e| db_err(&self.path, e))?;
                let step_meta: Option<String> = r.get(4).map_err(|e| db_err(&self.path, e))?;
                match kind {
                    KIND_STEP => {
                        let at = at_pos.unwrap_or(1).max(0) as usize;
                        let json = step_meta
                            .ok_or_else(|| Error::internal("missing step metadata in history"))?;
                        let mut step: Step = serde_json::from_str(&json).map_err(json_err)?;
                        if step.id.is_none() {
                            step.id = step_id.map(|v| v as u64);
                        }
                        steps.place_at(at, step);
                    }
                    KIND_UNDO => steps.back(at_pos.unwrap_or(0).max(0) as usize),
                    KIND_REDO => steps.forward(at_pos.unwrap_or(0).max(0) as usize),
                    KIND_MARK => {
                        let json = step_meta
                            .ok_or_else(|| Error::internal("missing mark metadata in history"))?;
                        let mark: Mark = serde_json::from_str(&json).map_err(json_err)?;
                        steps.mark(mark);
                    }
                    KIND_UNMARK => {
                        if let Some(id) = step_id {
                            steps.unmark(id as u64);
                        }
                    }
                    _ => {}
                }
            }
            *cache = StepsCache {
                path: self.path.clone(),
                generation: meta.generation,
                last_seq,
                steps,
            };
            Ok(true)
        })
    }

    pub(crate) fn read_all_in(conn: &Connection) -> Result<(Value, History, i64)> {
        let base_blob: Vec<u8> = conn
            .query_row("SELECT base_json FROM history_base WHERE id = 1", [], |r| {
                r.get(0)
            })
            .map_err(|_| Error::internal("no first project"))?;
        let base_raw = decompress_bytes(&base_blob)?;
        let base: Value = serde_json::from_slice(&base_raw).map_err(json_err)?;
        let mut tree: Tree<Entry> = Tree::default();
        let mut max_seq = 0i64;
        let mut stmt = conn
            .prepare_cached(
                "SELECT seq, kind, at_pos, step_id, step_meta, change_delta \
                 FROM history_events ORDER BY seq ASC",
            )
            .map_err(|e| Error::internal(e.to_string()))?;
        let mut rows = stmt.query([]).map_err(|e| Error::internal(e.to_string()))?;
        while let Some(r) = rows.next().map_err(|e| Error::internal(e.to_string()))? {
            max_seq = r.get(0).map_err(|e| Error::internal(e.to_string()))?;
            let kind: i64 = r.get(1).map_err(|e| Error::internal(e.to_string()))?;
            let at_pos: Option<i64> = r.get(2).map_err(|e| Error::internal(e.to_string()))?;
            let step_id: Option<i64> = r.get(3).map_err(|e| Error::internal(e.to_string()))?;
            let step_meta: Option<String> = r.get(4).map_err(|e| Error::internal(e.to_string()))?;
            let change_delta: Option<Vec<u8>> =
                r.get(5).map_err(|e| Error::internal(e.to_string()))?;
            match kind {
                KIND_STEP => {
                    let at = at_pos.unwrap_or(1).max(0) as usize;
                    let entry = parse_entry(step_meta, step_id, change_delta)?;
                    tree.place_at(at, entry);
                }
                KIND_UNDO => tree.back(at_pos.unwrap_or(0).max(0) as usize),
                KIND_REDO => tree.forward(at_pos.unwrap_or(0).max(0) as usize),
                KIND_MARK => {
                    let json = step_meta
                        .ok_or_else(|| Error::internal("missing mark metadata in history"))?;
                    let mark: Mark = serde_json::from_str(&json).map_err(json_err)?;
                    tree.mark(mark);
                }
                KIND_UNMARK => {
                    if let Some(id) = step_id {
                        tree.unmark(id as u64);
                    }
                }
                _ => {}
            }
        }
        Ok((base, History::of(tree), max_seq))
    }

    pub(crate) fn last_saved_in(conn: &Connection, dir: &Path) -> Option<Project> {
        let (mut json, h, _) = Self::read_all_in(conn).ok()?;
        for e in &h.undo {
            e.change.apply(&mut json, true).ok()?;
        }
        let mut project: Project = serde_json::from_value(json).ok()?;
        crate::store::resolve_paths(&mut project, dir);
        Some(project)
    }

    pub(crate) fn chain(&self, id: u64) -> Result<Option<Chain>> {
        let mut chain: Option<Chain> = self.read(|conn| {
            let (_, h, _) = Self::read_all_in(conn)?;
            Ok(crate::steps::chain(&h.branches, id)
                .map(|c| c.into_iter().map(|b| (b.after, b.steps.clone())).collect()))
        })?;
        if let Some(chain) = chain.as_mut() {
            let dir = self.dir();
            for (_, entries) in chain {
                for e in entries {
                    crate::store::resolve_change(&mut e.change, dir);
                }
            }
        }
        Ok(chain)
    }

    pub(crate) fn compact(&self) -> Result<()> {
        {
            let mut running = COMPACTING.lock().unwrap_or_else(|e| e.into_inner());
            if !running
                .get_or_insert_with(HashSet::new)
                .insert(self.path.clone())
            {
                return Ok(());
            }
        }
        let result = self.compact_to(HISTORY_CAP, || Ok(()));
        if let Some(running) = COMPACTING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            running.remove(&self.path);
        }
        result
    }

    fn compact_to(&self, limit: usize, hook: impl FnOnce() -> Result<()>) -> Result<()> {
        let (meta, mut base, mut h, snap_max_seq) = self.read(|conn| {
            let meta = Self::read_meta_in(conn)?;
            if meta.external_dirty || meta.steps <= limit {
                return Ok((meta, Value::Null, History::default(), 0));
            }
            let (base, h, max_seq) = Self::read_all_in(conn)?;
            Ok((meta, base, h, max_seq))
        })?;
        if meta.external_dirty || meta.steps <= limit {
            return Ok(());
        }
        let keep = limit - limit / 10;
        let next = meta
            .next
            .max(biggest(&h.undo, &h.branches).max(biggest(&h.redo, &[])) + 1);
        let reserve = h.branches.iter().map(size).sum::<usize>().min(limit / 10);
        let gone = h.undo.len().saturating_sub(keep - reserve);
        for e in h.undo.drain(..gone) {
            e.change.apply(&mut base, true).map_err(Error::internal)?;
        }
        h.branches.retain(|b| b.after >= gone);
        for b in &mut h.branches {
            b.after -= gone;
        }
        let main = h.undo.len() + h.redo.len();
        let named: HashSet<u64> = h.marks.iter().map(|m| m.step).collect();
        h.branches.sort_by_key(|b| {
            (
                std::cmp::Reverse(holds(b, &named)),
                std::cmp::Reverse(b.replaced),
            )
        });
        let mut room = keep.saturating_sub(main);
        h.branches.retain(|b| {
            let n = size(b);
            let fits = n <= room;
            room = room.saturating_sub(n);
            fits
        });
        h.branches.sort_by_key(|b| b.replaced);
        let mut ids = HashSet::new();
        ids_in(&h.undo, &h.branches, &mut ids);
        ids_in(&h.redo, &[], &mut ids);
        h.marks.retain(|m| match m.step {
            0 => gone == 0,
            id => ids.contains(&id),
        });
        hook()?;
        self.with_conn_mut(|conn| {
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|e| db_err(&self.path, e))?;
            let now_meta = Self::read_meta_in(&tx)?;
            if now_meta.generation != meta.generation || now_meta.external_dirty {
                return Ok(());
            }
            type RawEventRow = (
                i64,
                Option<i64>,
                Option<i64>,
                Option<String>,
                Option<String>,
                Option<Vec<u8>>,
            );
            let more: Vec<RawEventRow> = {
                let mut stmt = tx
                    .prepare_cached(
                        "SELECT kind, at_pos, step_id, label, step_meta, change_delta \
                         FROM history_events WHERE seq > ?1 ORDER BY seq ASC",
                    )
                    .map_err(|e| db_err(&self.path, e))?;
                let rows = stmt
                    .query_map(params![snap_max_seq], |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                        ))
                    })
                    .map_err(|e| db_err(&self.path, e))?;
                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| db_err(&self.path, e))?
            };
            if now_meta.undo < gone {
                return Ok(());
            }
            let (mut pos, mut redo) = (meta.undo, meta.redo);
            for (kind, at_pos, _, _, _, _) in &more {
                match *kind {
                    KIND_STEP => {
                        let at = at_pos.unwrap_or(1).max(0) as usize;
                        if at <= gone {
                            return Ok(());
                        }
                        pos = at;
                        redo = 0;
                    }
                    KIND_UNDO => {
                        let step = (at_pos.unwrap_or(0).max(0) as usize).min(pos);
                        pos -= step;
                        redo += step;
                        if pos < gone {
                            return Ok(());
                        }
                    }
                    KIND_REDO => {
                        let step = (at_pos.unwrap_or(0).max(0) as usize).min(redo);
                        redo -= step;
                        pos += step;
                    }
                    _ => {}
                }
            }
            let mut written = Self::write_whole_in(
                &tx,
                &base,
                &h,
                now_meta.generation + 1,
                Some(next.max(now_meta.next)),
                false,
            )?;
            if !more.is_empty() {
                let mut stmt = tx
                    .prepare_cached(
                        "INSERT INTO history_events \
                         (kind, at_pos, step_id, label, step_meta, change_delta) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    )
                    .map_err(|e| db_err(&self.path, e))?;
                for (kind, at_pos, step_id, label, step_meta, change_delta) in more {
                    let at_pos = match kind {
                        KIND_STEP => {
                            written.steps += 1;
                            at_pos.map(|p| (p.max(0) as usize).saturating_sub(gone) as i64)
                        }
                        _ => at_pos,
                    };
                    stmt.execute(params![
                        kind,
                        at_pos,
                        step_id,
                        label,
                        step_meta,
                        change_delta
                    ])
                    .map_err(|e| db_err(&self.path, e))?;
                }
                written.undo = now_meta.undo.saturating_sub(gone);
                written.redo = now_meta.redo;
                tx.execute(
                    "UPDATE history_meta SET undo_count = ?1, redo_count = ?2, total_steps = ?3 \
                     WHERE id = 1",
                    params![
                        written.undo as i64,
                        written.redo as i64,
                        written.steps as i64
                    ],
                )
                .map_err(|e| db_err(&self.path, e))?;
            }
            crate::store::gc_blobs(&tx)?;
            tx.commit().map_err(|e| db_err(&self.path, e))?;
            crate::store::settle(conn);
            Ok(())
        })
    }
}

fn parse_entry(
    step_meta: Option<String>,
    step_id: Option<i64>,
    change_delta: Option<Vec<u8>>,
) -> Result<Entry> {
    let json = step_meta.ok_or_else(|| Error::internal("missing step metadata in history"))?;
    let mut step: Step = serde_json::from_str(&json).map_err(json_err)?;
    if step.id.is_none() {
        step.id = step_id.map(|v| v as u64);
    }
    let delta_blob =
        change_delta.ok_or_else(|| Error::internal("missing step delta in history"))?;
    let delta_raw = decompress_bytes(&delta_blob)?;
    let change = serde_json::from_slice(&delta_raw).map_err(json_err)?;
    Ok(Entry { step, change })
}

fn read_entry_cached(
    conn: &Connection,
    path: &Path,
    dir: &Path,
    generation: u64,
    seq: i64,
    cache: Option<&EntryCache>,
) -> Result<Entry> {
    if let Some(c) = cache
        && let Some(hit) = c
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(generation, seq))
    {
        return Ok(hit.clone());
    }
    let (step_id, step_meta, change_delta): (Option<i64>, Option<String>, Option<Vec<u8>>) = conn
        .query_row(
            "SELECT step_id, step_meta, change_delta FROM history_events WHERE seq = ?1",
            params![seq],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|e| db_err(path, e))?;
    let mut entry = parse_entry(step_meta, step_id, change_delta)?;
    crate::store::resolve_change(&mut entry.change, dir);
    if let Some(c) = cache {
        c.lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((generation, seq), entry.clone());
    }
    Ok(entry)
}

fn emit_steps(
    stmt: &mut rusqlite::CachedStatement<'_>,
    offset: usize,
    steps: &[Entry],
    branches: &[Branch<Entry>],
    total: &mut usize,
    next: &mut u64,
    assign: bool,
) -> Result<()> {
    for i in 0..=steps.len() {
        for b in branches.iter().filter(|b| b.after == i) {
            emit_steps(stmt, offset + i, &b.steps, &b.branches, total, next, assign)?;
        }
        if let Some(e) = steps.get(i) {
            let mut e = e.clone();
            if assign && e.step.id.is_none() {
                e.step.id = Some(*next);
                *next += 1;
            }
            *total += 1;
            let step_json = serde_json::to_string(&e.step).map_err(json_err)?;
            let change_raw = serde_json::to_vec(&e.change).map_err(json_err)?;
            let change_blob = compress_bytes(&change_raw)?;
            stmt.execute(params![
                KIND_STEP,
                (offset + i + 1) as i64,
                e.step.id.map(|id| id as i64),
                e.step.label,
                step_json,
                change_blob,
            ])
            .map_err(|err| Error::internal(err.to_string()))?;
        }
    }
    Ok(())
}

fn size<T>(b: &Branch<T>) -> usize {
    b.steps.len() + b.branches.iter().map(size).sum::<usize>()
}

fn holds<T: Node>(b: &Branch<T>, ids: &HashSet<u64>) -> bool {
    b.steps
        .iter()
        .any(|s| s.id().is_some_and(|id| ids.contains(&id)))
        || b.branches.iter().any(|x| holds(x, ids))
}

fn ids_in<T: Node>(steps: &[T], branches: &[Branch<T>], out: &mut HashSet<u64>) {
    out.extend(steps.iter().filter_map(Node::id));
    for b in branches {
        ids_in(&b.steps, &b.branches, out);
    }
}

fn biggest<T: Node>(steps: &[T], branches: &[Branch<T>]) -> u64 {
    steps
        .iter()
        .filter_map(Node::id)
        .chain(branches.iter().map(|b| biggest(&b.steps, &b.branches)))
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::change::Change;
    use crate::model::Settings;
    use crate::store::Store;

    fn step(k: i64) -> Entry {
        Entry::new(
            format!("step {k}"),
            Change::between(&json!({"n": k - 1}), &json!({"n": k})).unwrap(),
        )
    }

    fn made(k: i64, id: u64, time: u64) -> Box<Entry> {
        let mut e = step(k);
        e.step.id = Some(id);
        e.step.time = Some(time);
        Box::new(e)
    }

    fn history(dir: &Path, n: i64) -> HistoryDb {
        let path = dir.join("p.vidcut");
        let project = Project::new(Settings {
            auto_format: false,
            ..Settings::default()
        });
        Store::create(&path, &project).unwrap();
        let db = HistoryDb::new(path);
        let h = History {
            undo: (1..=n).map(step).collect(),
            ..History::default()
        };
        db.with_conn_mut(|conn| {
            let tx = conn.transaction().unwrap();
            HistoryDb::write_whole_in(&tx, &json!({"n": 0}), &h, 1, None, true).unwrap();
            tx.commit().unwrap();
            Ok(())
        })
        .unwrap();
        db
    }

    fn old_history(dir: &Path, n: i64) -> HistoryDb {
        let path = dir.join("p.vidcut");
        let project = Project::new(Settings {
            auto_format: false,
            ..Settings::default()
        });
        Store::create(&path, &project).unwrap();
        let db = HistoryDb::new(path);
        let h = History {
            undo: (1..=n).map(step).collect(),
            ..History::default()
        };
        db.with_conn_mut(|conn| {
            let tx = conn.transaction().unwrap();
            HistoryDb::write_whole_in(&tx, &json!({"n": 0}), &h, 1, Some(1), false).unwrap();
            tx.commit().unwrap();
            Ok(())
        })
        .unwrap();
        db
    }

    fn save(db: &HistoryDb, moved: Vec<Moved>) {
        db.with_conn_mut(|conn| {
            let tx = conn.transaction().unwrap();
            let meta = HistoryDb::read_meta_in(&tx).unwrap();
            HistoryDb::append_in(&tx, db.dir(), &meta, &moved).unwrap();
            tx.commit().unwrap();
            Ok(())
        })
        .unwrap();
    }

    fn steps_of(db: &HistoryDb) -> Steps {
        let mut cache = StepsCache::default();
        db.steps_now(&mut cache).unwrap();
        cache.steps
    }

    fn labels(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.step.label.as_str()).collect()
    }

    #[test]
    fn the_step_to_undo_is_the_newest_at_its_place() {
        let dir = tempfile::tempdir().unwrap();
        let db = history(dir.path(), 2);
        save(&db, vec![Moved::Undo(1), Moved::Step(Box::new(step(3)))]);
        let (undo, _) = db.top(2, 0, None).unwrap();
        assert_eq!(labels(&undo), ["step 3", "step 1"]);
    }

    #[test]
    fn the_steps_to_redo_come_in_the_order_they_were_made() {
        let dir = tempfile::tempdir().unwrap();
        let db = history(dir.path(), 3);
        save(&db, vec![Moved::Undo(3)]);
        let (_, redo) = db.top(0, 3, None).unwrap();
        assert_eq!(labels(&redo), ["step 1", "step 2", "step 3"]);
    }

    #[test]
    fn a_compaction_keeps_90_percent_of_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let db = history(dir.path(), 11);
        db.compact_to(10, || Ok(())).unwrap();
        let meta = db.meta().unwrap();
        let (base, h, _) = db.read(HistoryDb::read_all_in).unwrap();
        assert_eq!((meta.undo, base), (9, json!({"n": 2})));
        assert_eq!(labels(&h.undo)[0], "step 3");
    }

    #[test]
    fn a_step_saved_during_a_compaction_stays() {
        let dir = tempfile::tempdir().unwrap();
        let db = history(dir.path(), 11);
        let db2 = db.clone();
        db.compact_to(10, || {
            save(&db2, vec![Moved::Step(Box::new(step(12)))]);
            Ok(())
        })
        .unwrap();
        let meta = db.meta().unwrap();
        let (undo, _) = db.top(2, 0, None).unwrap();
        assert_eq!((meta.undo, labels(&undo)), (10, vec!["step 12", "step 11"]));
    }

    #[test]
    fn a_step_written_before_steps_had_sources_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let db = history(dir.path(), 0);
        db.with_conn_mut(|conn| {
            let tx = conn.transaction().unwrap();
            let blob = compress_bytes(br#"{"set":{"was":0,"is":1}}"#).unwrap();
            tx.execute(
                "INSERT INTO history_events (kind, at_pos, label, step_meta, change_delta) \
                 VALUES (0, 1, 'old', '{\"label\":\"old\"}', ?1)",
                params![blob],
            )
            .unwrap();
            tx.execute(
                "UPDATE history_meta SET undo_count = 1, total_steps = 1 WHERE id = 1",
                [],
            )
            .unwrap();
            tx.commit().unwrap();
            Ok(())
        })
        .unwrap();
        assert_eq!(steps_of(&db).line, [Step::labeled("old")]);
    }

    #[test]
    fn a_step_by_a_client_kind_this_vidcut_doesnt_know_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let db = history(dir.path(), 0);
        db.with_conn_mut(|conn| {
            let tx = conn.transaction().unwrap();
            let blob = compress_bytes(br#"{"set":{"was":0,"is":1}}"#).unwrap();
            tx.execute(
                "INSERT INTO history_events (kind, at_pos, step_id, label, step_meta, change_delta) \
                 VALUES (0, 1, 1, 'new', '{\"label\":\"new\",\"by\":{\"kind\":\"robot\"}}', ?1)",
                params![blob],
            )
            .unwrap();
            tx.execute(
                "UPDATE history_meta SET undo_count = 1, total_steps = 1, next_step_id = 2 WHERE id = 1",
                [],
            )
            .unwrap();
            tx.commit().unwrap();
            Ok(())
        })
        .unwrap();
        let (undo, _) = db.top(1, 0, None).unwrap();
        assert_eq!(
            undo[0].step.by.as_ref().map(|c| c.kind),
            Some(crate::request::ClientKind::Other)
        );
    }

    #[test]
    fn steps_read_again_after_a_save_match_a_fresh_read() {
        let dir = tempfile::tempdir().unwrap();
        let db = history(dir.path(), 3);
        let mut cache = StepsCache::default();
        db.steps_now(&mut cache).unwrap();
        save(&db, vec![Moved::Undo(1), Moved::Step(Box::new(step(4)))]);
        db.steps_now(&mut cache).unwrap();
        let mut fresh = StepsCache::default();
        db.steps_now(&mut fresh).unwrap();
        assert_eq!(cache.steps, fresh.steps);
    }

    #[test]
    fn a_compaction_keeps_a_branch() {
        let dir = tempfile::tempdir().unwrap();
        let db = history(dir.path(), 11);
        save(&db, vec![Moved::Undo(1), Moved::Step(Box::new(step(12)))]);
        db.compact_to(10, || Ok(())).unwrap();
        let b = &steps_of(&db).branches[0];
        assert_eq!((b.after, b.steps[0].label.as_str()), (7, "step 11"));
    }

    #[test]
    fn a_compaction_leaves_room_for_the_next_step() {
        let dir = tempfile::tempdir().unwrap();
        let db = history(dir.path(), 11);
        save(&db, vec![Moved::Undo(1), Moved::Step(Box::new(step(12)))]);
        db.compact_to(10, || Ok(())).unwrap();
        assert_eq!(db.meta().unwrap().steps, 9);
    }

    #[test]
    fn a_compaction_gives_no_step_an_id_a_step_saved_meanwhile_has() {
        let dir = tempfile::tempdir().unwrap();
        let db = old_history(dir.path(), 11);
        save(&db, vec![Moved::Step(Box::new(step(12)))]);
        let db2 = db.clone();
        db.compact_to(10, || {
            save(&db2, vec![Moved::Step(Box::new(step(13)))]);
            Ok(())
        })
        .unwrap();
        let ids: Vec<u64> = steps_of(&db).line.iter().filter_map(|s| s.id).collect();
        assert_eq!(ids, [1, 2]);
    }

    #[test]
    fn a_compaction_that_drops_a_branch_doesnt_give_its_ids_again() {
        let dir = tempfile::tempdir().unwrap();
        let db = history(dir.path(), 11);
        save(
            &db,
            vec![
                Moved::Undo(2),
                Moved::Step(made(12, 12, 1)),
                Moved::Step(made(13, 13, 1)),
            ],
        );
        save(
            &db,
            vec![
                Moved::Undo(2),
                Moved::Step(made(10, 10, 1)),
                Moved::Step(made(11, 11, 1)),
            ],
        );
        db.compact_to(10, || Ok(())).unwrap();
        assert_eq!(db.meta().unwrap().next, 14);
    }

    #[test]
    fn a_compaction_that_drops_the_first_steps_drops_the_name_of_the_start() {
        let dir = tempfile::tempdir().unwrap();
        let db = history(dir.path(), 11);
        let start = Mark {
            step: 0,
            name: "Start".into(),
            time: None,
        };
        save(&db, vec![Moved::Mark(start)]);
        db.compact_to(10, || Ok(())).unwrap();
        assert_eq!(steps_of(&db).marks, []);
    }

    #[test]
    fn a_compaction_drops_the_names_of_the_steps_it_drops() {
        let dir = tempfile::tempdir().unwrap();
        let db = history(dir.path(), 11);
        let first = Mark {
            step: 1,
            name: "First".into(),
            time: None,
        };
        save(&db, vec![Moved::Mark(first)]);
        db.compact_to(10, || Ok(())).unwrap();
        assert_eq!(steps_of(&db).marks, []);
    }

    #[test]
    fn a_compaction_keeps_a_named_branch_before_a_newer_one() {
        let dir = tempfile::tempdir().unwrap();
        let db = history(dir.path(), 11);
        save(&db, vec![Moved::Undo(1), Moved::Step(made(12, 12, 2))]);
        save(&db, vec![Moved::Undo(1), Moved::Step(made(13, 13, 3))]);
        let kept = Mark {
            step: 11,
            name: "Keep".into(),
            time: None,
        };
        save(&db, vec![Moved::Mark(kept)]);
        db.compact_to(10, || Ok(())).unwrap();
        let labels: Vec<String> = steps_of(&db)
            .branches
            .iter()
            .map(|b| b.steps[0].label.clone())
            .collect();
        assert_eq!(labels, ["step 11"]);
    }

    #[test]
    fn a_saved_step_gets_the_next_id() {
        let dir = tempfile::tempdir().unwrap();
        let db = history(dir.path(), 3);
        save(&db, vec![Moved::Step(Box::new(step(4)))]);
        let (undo, _) = db.top(1, 0, None).unwrap();
        assert_eq!(undo[0].step.id, Some(4));
    }

    #[test]
    fn compaction_bails_when_external_dirty_is_set() {
        let dir = tempfile::tempdir().unwrap();
        let db = history(dir.path(), 11);
        db.read(|c| {
            c.execute(
                "UPDATE history_meta SET external_dirty = 1 WHERE id = 1",
                [],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
        db.compact_to(10, || Ok(())).unwrap();
        let meta = db.meta().unwrap();
        assert_eq!(
            (meta.generation, meta.steps, meta.external_dirty),
            (1, 11, true)
        );
    }

    #[test]
    fn compaction_bails_when_concurrent_undo_reaches_trimmed_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let db = history(dir.path(), 11);
        let db2 = db.clone();
        db.compact_to(10, || {
            save(&db2, vec![Moved::Undo(10)]);
            Ok(())
        })
        .unwrap();
        let meta = db.meta().unwrap();
        assert_eq!((meta.generation, meta.undo, meta.redo), (1, 1, 10));
        let (_, redo) = db.top(0, 10, None).unwrap();
        assert_eq!(redo.len(), 10);
    }
}
